import {
  Fragment,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { Canvas, createPortal, useFrame, useThree } from '@react-three/fiber';
import {
  Environment,
  Lightformer,
  OrbitControls,
  Outlines,
  Preload,
  useGLTF,
} from '@react-three/drei';
import CircularProgress from '@mui/material/CircularProgress';
import { useTheme } from '@mui/material/styles';
import {
  Group,
  MathUtils,
  PerspectiveCamera,
  Quaternion,
  Vector3,
  type Material,
  type Mesh,
  type MeshBasicMaterial,
  type Object3D,
} from 'three';

import glbUrl from '@/assets/robot-3d/reachy_flasher.glb?url';
import { SHOTS, type PartState, type Shot, type ShotId, type Vec3 } from './shots';

const DRACO_PATH = '/draco/';
useGLTF.preload(glbUrl, DRACO_PATH);

/** Whether the dev tuning HUD is even reachable (only in `vite dev`). */
const DEV_TUNING = import.meta.env.DEV;

// --------------------------------------------------------------------------
// Model
// --------------------------------------------------------------------------

/** GLTFLoader sanitizes names (spaces/dots -> non-alphanumerics stripped), so
 * "Shell Head front" becomes "Shell_Head_front" on load. Normalize both sides
 * when matching part / material names. */
const norm = (s: string): string => s.toLowerCase().replace(/[^a-z0-9]/g, '');

// Antenna posing, matching the mobile/desktop app convention exactly:
//   bone `Antenna.[LR].002`, rotated about its LOCAL Z axis, in radians,
//   quaternion = restQuaternion * Rz(ANT_SIGN * angle)  (relative to rest pose).
// `antennas` is [rightRad, leftRad]. Rest quaternions are captured once (stored
// in the bone's userData) so re-running on drei's shared cached scene is
// idempotent and never compounds the rotation.
const ANT_AXIS = new Vector3(0, 0, 1);
const ANT_SIGN = -1;
// Target joint angles [right, left] in radians. The rest pose is antennas ~up;
// the app's SLEEP pose (~+-175 deg) folds them fully down.
const ANTENNA_POSE: [number, number] = [-3.35, 3.35];

/** Pose one antenna bone the way the mobile app does: capture its rest
 * quaternion once, then set `quaternion = rest * Rz(ANT_SIGN * angle)`. */
function poseAntenna(bone: Object3D | undefined, angle: number): void {
  if (!bone) return;
  const ud = bone.userData as { __restQuat?: Quaternion };
  if (!ud.__restQuat) ud.__restQuat = bone.quaternion.clone();
  const rot = new Quaternion().setFromAxisAngle(ANT_AXIS, ANT_SIGN * angle);
  bone.quaternion.copy(ud.__restQuat).multiply(rot);
  bone.updateMatrixWorld(true);
}

// USB-C cable: we now use the REAL connector mesh imported from the model
// (tagged `plug` in the glb extras) rather than a procedural box. On the USB
// shot, once the camera settles, that connector slides from an offset
// (unplugged) into the CM4 USB-C port along its own axis (+X / -Z, glb Y-up).
const CABLE_OUT_DIR = new Vector3(0.707, 0, -0.707).normalize();
const CABLE_OUT_DIST = 0.2; // travel of the connector: far enough to slide fully
// OUT of the tight board frame before it's hidden (and to enter from off-screen
// when plugging in), so it never visibly pops away mid-frame.
const CABLE_LAMBDA = 3.5; // slide ease speed when PLUGGING IN (lower = slower / smoother)
const CABLE_UNPLUG_LAMBDA = 2.0; // slower ease when UNPLUGGING (out) so the pull-out reads gentler

// SW1 slide switch "toggle": the actuator is modelled in the DEBUG position
// (rest). Once the camera settles on the SW1 shot we slide it ONCE to DOWNLOAD
// (Blender +Y -> glTF -Z) and hold it there - a real toggle, not an attention
// oscillation. The move is fairly DIRECT (snappy) but still lerped, not a snap.
const SW1_DIR = new Vector3(0, 0, -1); // rest(DEBUG) -> DOWNLOAD
const SW1_THROW = -0.0025;
const SW1_LAMBDA = 14; // higher = more direct/snappy lerp toward the target

// Small/subtle parts (the SW1 slide, the head screws) are also highlighted with
// a crisp, thick OUTLINE in the theme primary. This uses drei's <Outlines>
// (inverted-hull), NOT a post-process: a back-side shell expanded along the
// normals. See <StepHighlight> in ReachyStage. The highlight is not shown for
// the whole step: it fades in a bit AFTER the step starts and fades out again a
// bit BEFORE it would end, as a transient attention cue.
const SW1_OUTLINE_COLOR = 0xff9500; // theme primary (see theme.ts ACCENT)
const SW1_OUTLINE_PX = 6; // switch outline thickness in SCREEN PIXELS (crisp, zoom-independent)
const SCREW_OUTLINE_PX = 4; // screw outline thickness (smaller parts -> thinner line)
// Step highlights appear a short, FIXED beat after the step becomes active (not
// tied to when the part moves), hold for the rest of the step, then fade out as
// soon as we leave the step (the camera flies to the next shot / the highlight
// deactivates). The opacity is eased, so both edges - and the "solid" gate that
// waits out a fading target (see meshesSolid) - fade instead of popping.
const HL_ENTRY = 0.4; // s after the step becomes active before the highlight fades IN
const HL_FADE_LAMBDA = 7; // opacity ease speed (~0.4 s fade in on entry / out on step change)

// Head-shell screws: on the open-head shot they back out STRAIGHT BACKWARD
// (glb world +Z, which is the shell's rear-face normal so they slide cleanly
// out of their holes). Matched by NODE NAME - the earlier `screw` glb tag
// pointed at the wrong two screws, so we ignore it and target these four.
const SCREW_THROW = 0.24; // metres the screws travel out
const SCREW_DIR = new Vector3(0, 0, 1); // straight backward (glb world +Z)
const SCREW_NODE_NAMES = ['Vis large.010', 'Vis large.011', 'Vis large.012', 'Vis large.013'].map(
  norm,
);
// Staggered "unscrew": a master clock ramps 0..SCREW_TOTAL while engaged; each
// screw i eases out over its own [i*STAGGER, i*STAGGER + DUR] slice (slight
// overlap since STAGGER < DUR). Total window is kept ~equal to the previous
// single exponential eouase (~0.8 s) so the overall timing is unchanged.
const SCREW_DUR = 0.26; // per-screw ease duration (s)
const SCREW_STAGGER = 0.18; // delay between consecutive screws starting (s)
const SCREW_TOTAL = 3 * SCREW_STAGGER + SCREW_DUR; // 0.8 s for 4 screws

/** easeOutCubic: quick start, gentle settle - reads as a deliberate pull-out. */
function easeOutCubic(t: number): number {
  const x = 1 - t;
  return 1 - x * x * x;
}

// Front-face head shell: opening the head is split across TWO steps, and the
// SLIDE (translation) is deliberately decoupled from the FADE (opacity):
//   - On the SCREW step, once the 4 screws have backed out the whole front unit
//     SLIDES FORWARD (glb world -Z, the front-face normal) and STAYS visible, so
//     the lift reads at full amplitude (nothing fades mid-move).
//   - Only when animating to the NEXT step does it FADE OUT.
// The two are driven by independent progresses (slide tied to `screwsOut`, fade
// tied to `headOpen`), so the close wizard reverses both for free: the shell
// fades back in, then slides home.
const FHEAD_WORLD_DIR = new Vector3(0, 0, -1); // forward (front-face normal); screws back out +Z
const FHEAD_THROW = 0.4; // world units the shell slides forward

/** Returns the material name(s) of a mesh (materials survive the rig, so we can
 * hide the eyes by their shared `Glass_Eyes` material rather than by object
 * name -- the eye lenses are unnamed `Cylinder.NNN` meshes). */
function meshTags(mesh: Mesh): string[] {
  const tags = [norm(mesh.name)];
  const mat = mesh.material as Material | Material[] | undefined;
  if (Array.isArray(mat)) for (const m of mat) tags.push(norm(m?.name ?? ''));
  else if (mat) tags.push(norm(mat.name ?? ''));
  return tags;
}

/** True when the mesh (or any ancestor) belongs to the head's FRONT-FACE unit -
 * front shell + eye lenses + camera modules + their electronics - tagged `fhead`
 * at export time and surfaced by three's GLTFLoader as node `userData`. */
function isFrontHead(o: Object3D): boolean {
  let n: Object3D | null = o;
  while (n) {
    if ((n.userData as { fhead?: number } | undefined)?.fhead) return true;
    n = n.parent;
  }
  return false;
}

/** Loads the robot once and toggles per-shot part visibility (e.g. lift the
 * head shell so the inner board is exposed on the close-ups). The rig is kept
 * intact (skeleton + bone-parented antennas); the antennas are aimed downward
 * at runtime -- like the mobile app poses the rig -- not baked into the mesh. */
function RobotModel({
  hiddenParts,
  headOpen,
  screwsOut,
  sw1Download,
  snapKey,
  dwellRef,
  onPick,
  onReady,
  onSw1Meshes,
  onScrewMeshes,
  onFheadMeshes,
}: {
  hiddenParts?: string[];
  /** Target part states (see PartState). Each part eases toward its target after
   * the camera settles; the animation DIRECTION follows the state change. */
  headOpen?: boolean;
  screwsOut?: boolean;
  sw1Download?: boolean;
  /** Changes when the wizard (re)starts; parts snap to their target instantly
   * instead of animating, so a fresh wizard opens in its correct state. */
  snapKey?: string;
  dwellRef?: { current: number };
  onPick?: (p: Vec3) => void;
  onReady?: () => void;
  onSw1Meshes?: (objs: Object3D[]) => void;
  onScrewMeshes?: (objs: Object3D[]) => void;
  onFheadMeshes?: (objs: Object3D[]) => void;
}) {
  const { scene } = useGLTF(glbUrl, DRACO_PATH);
  const invalidate = useThree((s) => s.invalidate);
  const groupRef = useRef<Group>(null);
  const screws = useRef<{ node: Object3D; rest: Vector3; dir: Vector3; key: string }[]>([]);
  const screwT = useRef(0); // master clock (s), 0 = all seated (in), SCREW_TOTAL = all out
  const sw1Rig = useRef<Group | null>(null);
  const sw1P = useRef(0); // slide progress: 0 = NORMAL/DEBUG (rest), 1 = DOWNLOAD
  const snapRef = useRef<string | undefined>(undefined); // last-seen snapKey
  // Front-face head: it FADES in/out (opacity) with a delay (see FHEAD_DELAY)
  // rather than popping instantly. We own cloned materials so the fade never
  // bleeds onto other meshes sharing the same source material. `fheadOpacity` is
  // the current applied alpha (1 = fully shown), `fheadApplied` the last written.
  const fheadMeshes = useRef<Object3D[]>([]);
  const fheadMats = useRef<Material[]>([]);
  // Top-most fhead nodes (parent is NOT fhead) with their pristine rest position
  // and forward slide direction, so the whole unit translates together without
  // compounding on nested meshes.
  const fheadRoots = useRef<{ node: Object3D; rest: Vector3; dir: Vector3 }[]>([]);
  const fheadFade = useRef(0); // opacity progress: 0 = fully shown, 1 = gone (tied to headOpen)
  const fheadSlide = useRef(0); // slide progress: 0 = home, 1 = slid forward (tied to screwsOut)
  const fheadApplied = useRef(-1); // last-written opacity

  // Point the two flexible antennas straight down (they rest pointing up) by
  // driving the rig from JS - the glb stays an untouched rest-pose export, we
  // just aim each antenna's root bone at world -Y (see aimAntennaDown). The
  // spring + plastic tip are bone-parented under the chain and follow rigidly.
  // Pose the antennas by JOINT ANGLE, exactly like the mobile/desktop app:
  // rotate bones Antenna.[LR].002 about local Z by the target angle (relative to
  // the captured rest pose). `antennas` is [right, left].
  useEffect(() => {
    const byName: Record<string, Object3D> = {};
    scene.traverse((o) => {
      if (o.name) byName[norm(o.name)] = o;
    });
    poseAntenna(byName['antennar002'], ANTENNA_POSE[0]); // right
    poseAntenna(byName['antennal002'], ANTENNA_POSE[1]); // left
    invalidate();
    // The model is loaded AND the antennas are in their final pose: signal the
    // stage it can drop the loading spinner and reveal the scene.
    onReady?.();
  }, [scene, invalidate, onReady]);

  // Per-shot hiding: match normalized object AND material names (see meshTags),
  // plus the whole front-face unit when requested (meshes tagged `fhead` in the
  // glb `extras`, surfaced by three as userData.fhead - see export_gltf).
  useEffect(() => {
    const tokens = (hiddenParts ?? []).map(norm).filter(Boolean);
    scene.traverse((o) => {
      const mesh = o as Mesh;
      if (!mesh.isMesh) return;
      // The USB-C connector's visibility is owned by UsbCable (it only shows on
      // the USB shot); never let the per-shot pass touch it.
      if ((mesh.userData as { plug?: number }).plug) return;
      // The front-face unit's visibility is owned by the delayed useFrame toggle
      // (see FHEAD_DELAY); skip it here so it isn't reset instantly on step change.
      if (isFrontHead(mesh)) return;
      const tags = meshTags(mesh);
      let hidden = tokens.some((t) => tags.some((tag) => tag.includes(t)));
      // Always hide any wired cable in the model (e.g. the internal "FFC cable"):
      // the only cable we ever show is the procedural USB-C one (see UsbCable).
      if (norm(mesh.name).includes('cable')) hidden = true;
      mesh.visible = !hidden;
    });
    invalidate();
  }, [scene, hiddenParts, invalidate]);

  // Collect the front-face head meshes once and CLONE their materials so we can
  // animate opacity on them alone (the source materials may be shared with other
  // meshes). Their delayed fade is driven in useFrame (see FHEAD_DELAY).
  useEffect(() => {
    const meshes: Object3D[] = [];
    const mats = new Set<Material>();
    // Clone (so opacity isn't shared) and mark each material transparent ONCE,
    // up front. We never toggle `.transparent` afterwards - flipping it at the
    // start of the fade is what caused the visible flicker (pass reshuffle /
    // implicit recompile). Instead we only animate `.opacity` and drive
    // `.depthWrite` (below), which is cheap and artefact-free.
    const prep = (m: Material) => {
      const c = m.clone();
      c.transparent = true;
      c.depthWrite = true; // starts fully opaque
      mats.add(c);
      return c;
    };
    scene.traverse((o) => {
      const mesh = o as Mesh;
      if (!mesh.isMesh || !isFrontHead(o)) return;
      meshes.push(o);
      if (Array.isArray(mesh.material)) {
        mesh.material = mesh.material.map((m) => prep(m as Material));
      } else if (mesh.material) {
        mesh.material = prep(mesh.material as Material);
      }
    });
    fheadMeshes.current = meshes;
    fheadMats.current = [...mats];

    // Capture the top-most fhead nodes (a node whose parent is NOT fhead) so we
    // slide the whole unit as one. Rest/dir are pristine + idempotent (like the
    // screws): re-running the effect while the shell is slid out must not bake
    // the offset into the rest pose.
    const roots: { node: Object3D; rest: Vector3; dir: Vector3 }[] = [];
    scene.traverse((o) => {
      if (!isFrontHead(o)) return;
      if (o.parent && isFrontHead(o.parent)) return; // keep only the top-most tagged node
      const ud = o.userData as { __fheadRest?: Vector3; __fheadDir?: Vector3 };
      if (!ud.__fheadRest) {
        ud.__fheadRest = o.position.clone();
        const d = FHEAD_WORLD_DIR.clone();
        // Express the world-space forward direction in the node's PARENT-local
        // frame, so bone/group-parented nodes still slide forward in world space.
        if (o.parent) d.applyQuaternion(o.parent.getWorldQuaternion(new Quaternion()).invert());
        ud.__fheadDir = d.normalize();
      }
      roots.push({ node: o, rest: ud.__fheadRest.clone(), dir: ud.__fheadDir!.clone() });
    });
    fheadRoots.current = roots;

    // Surface the SHELL meshes (front cover only, not the eyes/cameras/board) so
    // the open/close-head step can outline-highlight it. Fall back to the whole
    // front unit if the shell isn't separately named.
    const shell = meshes.filter((m) => norm(m.name).includes('shell'));
    onFheadMeshes?.(shell.length ? shell : meshes);
  }, [scene, onFheadMeshes]);

  // Group the SW1 switch body (tagged `sw1`) under one pivot so we can slide it.
  useEffect(() => {
    let rig = scene.getObjectByName('__Sw1Rig') as Group | undefined;
    if (!rig) {
      rig = new Group();
      rig.name = '__Sw1Rig';
      scene.add(rig);
      const parts: Object3D[] = [];
      scene.traverse((o) => {
        // A multi-material mesh is split into primitives and represented by three
        // as a GROUP carrying the `sw1` extras (its child meshes don't) - so match
        // on userData.sw1 regardless of node type, like the USB connector.
        if ((o.userData as { sw1?: number })?.sw1) parts.push(o);
      });
      for (const p of parts) rig.attach(p);
    }
    // Report the switch's actual meshes up so the <Outlines> hull can be portaled
    // onto each of them on the SW1 step (re-collected on hot remount too).
    const meshes: Object3D[] = [];
    rig.traverse((o) => {
      if ((o as Mesh).isMesh) meshes.push(o);
    });
    onSw1Meshes?.(meshes);
    sw1Rig.current = rig;
    invalidate();
  }, [scene, invalidate, onSw1Meshes]);

  // Collect the 4 head-shell screws (matched by NODE NAME) once, capturing each
  // one's rest position and its "back out" direction: straight backward (+Z in
  // the glb world frame), expressed in the node's PARENT-local space
  // (node.position is local) so bone-parented screws still travel rearward in
  // world space. Ordered .010 -> .011 -> .012 -> .013 so the stagger is stable.
  useEffect(() => {
    const want = new Set(SCREW_NODE_NAMES);
    const list: { node: Object3D; rest: Vector3; dir: Vector3; key: string }[] = [];
    scene.traverse((o) => {
      if (!(o as Mesh).isMesh) return;
      const key = norm(o.name);
      if (!want.has(key)) return;
      // Idempotent pristine rest/dir (see UsbCable): if the effect re-runs while
      // a screw is backed out, recapturing o.position would bake in the offset
      // and the screw would drift further "out" on every re-run.
      const ud = o.userData as { __restPos?: Vector3; __slideDir?: Vector3 };
      if (!ud.__restPos) {
        ud.__restPos = o.position.clone();
        const d = SCREW_DIR.clone();
        if (o.parent) d.applyQuaternion(o.parent.getWorldQuaternion(new Quaternion()).invert());
        ud.__slideDir = d.normalize();
      }
      list.push({ node: o, rest: ud.__restPos.clone(), dir: ud.__slideDir!.clone(), key });
    });
    list.sort((a, b) => SCREW_NODE_NAMES.indexOf(a.key) - SCREW_NODE_NAMES.indexOf(b.key));
    screws.current = list;
    // Surface the screw meshes so the <StepHighlight> can outline them.
    onScrewMeshes?.(list.map((s) => s.node));
    invalidate();
  }, [scene, invalidate, onScrewMeshes]);

  // Every movable part just EASES toward its declared target after the camera has
  // settled (its own per-part dwell delay). The animation direction is implicit
  // in how the target changes between steps, so there is no forward/reverse,
  // hold, or snap-out bookkeeping. On a wizard (re)start (snapKey change) the
  // parts jump straight to their targets so a fresh wizard opens correctly posed.
  useFrame((_, dt) => {
    const step = Math.min(dt, 0.1);
    const dwell = dwellRef?.current ?? Infinity;
    const snap = snapRef.current !== snapKey;
    if (snap) snapRef.current = snapKey;

    // Targets (glb-space values) for each part.
    const fadeTgt = headOpen ? 1 : 0; // front-head opacity progress (1 = gone)
    const slideTgt = screwsOut ? 1 : 0; // front-head slide progress (1 = slid forward)
    const swTgt = sw1Download ? 1 : 0; // switch slide
    const scTgt = screwsOut ? SCREW_TOTAL : 0; // screw clock

    // --- Front head FADE (opacity) - tied to headOpen ---
    // Only the *fade* follows headOpen, and headOpen only flips on the step AFTER
    // the screws come out. So within the screw step the shell stays fully opaque
    // (it just slides, below); it dissolves only as we animate to the next step.
    if (snap) fheadFade.current = fadeTgt;
    else if (dwell >= FHEAD_DELAY && fheadFade.current !== fadeTgt) {
      let f = fheadFade.current + (fadeTgt - fheadFade.current) * (1 - Math.exp(-FHEAD_LAMBDA * step));
      if (Math.abs(fadeTgt - f) < 0.001) f = fadeTgt;
      fheadFade.current = f;
      invalidate();
    }
    const op = 1 - fheadFade.current;
    if (fheadApplied.current !== op) {
      const opaque = op >= 0.999;
      for (const mat of fheadMats.current) {
        mat.opacity = op;
        // `.transparent` stays true forever (set at clone time); only write depth
        // when fully solid so the fading shell doesn't self-sort/flicker.
        mat.depthWrite = opaque;
      }
      for (const m of fheadMeshes.current) m.visible = op > 0.001;
      fheadApplied.current = op;
    }

    // --- Front head SLIDE (translation) - tied to screwsOut, after the screws ---
    // Delayed past the screws' unscrew window so, within one step, the shell first
    // unscrews then visibly lifts forward - shown at full amplitude since the fade
    // is deferred to the next step. On close (screwsOut -> false) it slides home,
    // after the shell has faded back in.
    if (snap) fheadSlide.current = slideTgt;
    else if (dwell >= FHEAD_SLIDE_DELAY && fheadSlide.current !== slideTgt) {
      let s =
        fheadSlide.current + (slideTgt - fheadSlide.current) * (1 - Math.exp(-FHEAD_SLIDE_LAMBDA * step));
      if (Math.abs(slideTgt - s) < 0.001) s = slideTgt;
      fheadSlide.current = s;
      invalidate();
    }
    {
      const off = fheadSlide.current * FHEAD_THROW;
      const roots = fheadRoots.current;
      for (let i = 0; i < roots.length; i++) {
        roots[i].node.position.copy(roots[i].rest).addScaledVector(roots[i].dir, off);
      }
    }

    // --- SW1 switch: slide toward target after SW1_DELAY ---
    const sw = sw1Rig.current;
    if (sw) {
      if (snap) sw1P.current = swTgt;
      else if (dwell >= SW1_DELAY)
        sw1P.current += (swTgt - sw1P.current) * (1 - Math.exp(-SW1_LAMBDA * step));
      sw.position.copy(SW1_DIR).multiplyScalar(sw1P.current * SW1_THROW);
    }

    // --- Head screws ---
    // The screws ALWAYS ease toward their target (staggered, after SCREW_DELAY),
    // on every step - not just the screw shot. Easing everywhere means an
    // interrupted transition just keeps easing from wherever it was to the new
    // target (no jump); the master clock is clamped to [0, SCREW_TOTAL] and the
    // rest pose is pristine (see the idempotent capture above), so no navigation
    // pattern can leave a screw stuck part-way or drifting "too far out". Only a
    // wizard (re)start snaps them, so a freshly-shown wizard opens correctly posed.
    if (snap) screwT.current = scTgt;
    else if (dwell >= SCREW_DELAY) {
      const d = MathUtils.clamp(scTgt - screwT.current, -step, step);
      screwT.current = MathUtils.clamp(screwT.current + d, 0, SCREW_TOTAL);
    }
    const list = screws.current;
    for (let i = 0; i < list.length; i++) {
      const local = MathUtils.clamp((screwT.current - i * SCREW_STAGGER) / SCREW_DUR, 0, 1);
      const off = easeOutCubic(local) * SCREW_THROW;
      list[i].node.position.copy(list[i].rest).addScaledVector(list[i].dir, off);
    }
  });

  return (
    <group
      ref={groupRef}
      onClick={
        onPick
          ? (e) => {
              e.stopPropagation();
              onPick([
                +e.point.x.toFixed(3),
                +e.point.y.toFixed(3),
                +e.point.z.toFixed(3),
              ]);
            }
          : undefined
      }
    >
      <primitive object={scene} />
    </group>
  );
}

// --------------------------------------------------------------------------
// USB-C cable (real connector, driven)
// --------------------------------------------------------------------------


/** Drives the REAL USB-C connector imported from the model (tagged `plug`),
 * animated in place (not reparented) so it keeps its material. It simply eases
 * toward its target (`plugged`): seated when plugged, slid out along its axis and
 * hidden when unplugged. The direction (plug in vs unplug) follows from how
 * `plugged` changes between steps; on a wizard (re)start it snaps to target. */
function UsbCable({
  plugged,
  snapKey,
  dwellRef,
  onMeshes,
}: {
  plugged?: boolean;
  snapKey?: string;
  dwellRef?: { current: number };
  /** Surfaces the connector's (and attached wire's) meshes so the USB step can
   * outline-highlight them. */
  onMeshes?: (objs: Object3D[]) => void;
}) {
  const { scene } = useGLTF(glbUrl, DRACO_PATH);
  const invalidate = useThree((s) => s.invalidate);
  const plug = useRef<Object3D | null>(null);
  const rest = useRef(new Vector3());
  const dir = useRef(CABLE_OUT_DIR.clone()); // slide axis in the plug's PARENT-local space
  const t = useRef(1); // 1 = unplugged (offset out), 0 = seated
  const snapRef = useRef<string | undefined>(undefined);

  // Locate the connector once, capture its rest position and its local slide
  // axis (world CABLE_OUT_DIR expressed in the node's parent frame), and hide it
  // until the USB shot reveals it.
  useEffect(() => {
    // The connector's mesh has several primitives, so three represents it as a
    // GROUP (which carries the `plug` extras) with child meshes that don't - so
    // match on userData.plug regardless of type and drive that group.
    const found: Object3D[] = [];
    scene.traverse((o) => {
      if ((o.userData as { plug?: number })?.plug) found.push(o);
    });
    const node = found[0] ?? null;
    plug.current = node;
    if (node) {
      // Capture the PRISTINE seated rest pose + slide axis exactly ONCE and
      // stash them on the node (like the antennas' __restQuat). The effect can
      // re-run (react-refresh/HMR, remounts) while useFrame has already slid the
      // connector to its unplugged offset; recapturing node.position then would
      // bake that offset into `rest`, so seating (t=0) would leave it 0.3 m away
      // and off the tight board frame - i.e. the cable would never visibly
      // "arrive". Reusing the stored pristine values keeps it idempotent.
      const ud = node.userData as { __restPos?: Vector3; __slideDir?: Vector3 };
      if (!ud.__restPos) {
        ud.__restPos = node.position.clone();
        const d = CABLE_OUT_DIR.clone();
        if (node.parent)
          d.applyQuaternion(node.parent.getWorldQuaternion(new Quaternion()).invert());
        ud.__slideDir = d.normalize();
      }
      rest.current.copy(ud.__restPos);
      dir.current.copy(ud.__slideDir!);
      node.visible = false;
      // Surface the connector (+ wire) meshes for the USB-step highlight.
      const meshes: Object3D[] = [];
      node.traverse((o) => {
        if ((o as Mesh).isMesh) meshes.push(o);
      });
      onMeshes?.(meshes);
    }
    invalidate();
  }, [scene, invalidate, onMeshes]);

  useFrame((_, dt) => {
    const node = plug.current;
    if (!node) return;
    const tgt = plugged ? 0 : 1; // 0 = seated, 1 = unplugged (out)
    // Unplugging (moving toward the out pose) eases slower than plugging in.
    const lambda = tgt === 1 ? CABLE_UNPLUG_LAMBDA : CABLE_LAMBDA;
    const a = 1 - Math.exp(-lambda * Math.min(dt, 0.1));
    const past = (dwellRef?.current ?? 0) >= CABLE_DELAY;
    if (snapRef.current !== snapKey) {
      snapRef.current = snapKey;
      t.current = tgt; // jump to target on a wizard (re)start
    } else if (past) {
      t.current += (tgt - t.current) * a; // ease toward target after the dwell
    }
    // Visible unless essentially fully unplugged (so it "appears" as it slides
    // in, and vanishes only once fully out).
    node.visible = t.current < 0.98;
    node.position.copy(rest.current).addScaledVector(dir.current, t.current * CABLE_OUT_DIST);
  });

  return null;
}

// --------------------------------------------------------------------------
// Transient outline highlight (switch, screws)
// --------------------------------------------------------------------------

/** True only when every target mesh is (still) solid enough to occlude its own
 * inverted-hull outline. The rim look relies on the object WRITING DEPTH so the
 * back-side hull is occluded except at the silhouette; a mesh mid-fade
 * (transparent with depthWrite off - e.g. the head shell fading back in on the
 * close step) occludes nothing, so the hull renders as a filled silhouette over
 * the whole object. Keying on `depthWrite` (not an opacity threshold) matches
 * that occlusion exactly, with no in-between window where the blob flashes. */
function meshesSolid(objs: Object3D[]): boolean {
  for (const o of objs) {
    const mat = (o as Mesh).material as Material | Material[] | undefined;
    const mats = Array.isArray(mat) ? mat : mat ? [mat] : [];
    for (const m of mats) {
      if (m.transparent && !m.depthWrite) return false;
    }
  }
  return true;
}

/** Portals a drei <Outlines> (crisp inverted-hull) onto each target mesh and
 * eases its opacity with a uniform, step-relative rule (see HL_ENTRY):
 *   - fades IN a fixed beat after the step becomes active (HL_ENTRY),
 *   - holds for the rest of the step,
 *   - fades OUT as soon as the step deactivates (we fly to the next shot).
 * A target that can't yet occlude its own hull (mid-fade; see meshesSolid) keeps
 * the outline suppressed until it turns solid - and because the SAME eased
 * opacity drives that, it fades in smoothly then too. Opacity is throttled into
 * state so only this small subtree re-renders (not the whole stage). */
function StepHighlight({
  meshes,
  active,
  dwellRef,
  thickness,
  entryAt = HL_ENTRY,
}: {
  meshes: Object3D[];
  active: boolean;
  dwellRef: { current: number };
  thickness: number;
  /** Seconds after the step becomes active before the highlight fades IN. */
  entryAt?: number;
}) {
  const [op, setOp] = useState(0);
  const easedRef = useRef(0); // continuous eased opacity (drives the fade both ways)
  const emittedRef = useRef(0); // last value pushed to state (throttle)
  useFrame((_, dt) => {
    // Shown once we're a beat into the step AND the target is solid enough to
    // occlude its hull; cleared instantly when the step deactivates. The ease
    // below turns each of these edges into a fade, never a pop.
    const shown = active && dwellRef.current >= entryAt && meshesSolid(meshes);
    const target = shown ? 1 : 0;
    easedRef.current += (target - easedRef.current) * (1 - Math.exp(-HL_FADE_LAMBDA * Math.min(dt, 0.1)));
    const v = easedRef.current < 0.003 ? 0 : Math.min(1, easedRef.current);
    // Throttle: only re-render when the change is visible.
    if (Math.abs(v - emittedRef.current) > 0.02 || (v === 0 && emittedRef.current !== 0)) {
      emittedRef.current = v;
      setOp(v);
    }
  });
  // Keep the outline hulls MOUNTED at all times (invisible at opacity 0) rather
  // than mounting them only when active: this compiles their (inverted-hull)
  // shader up front, so the first time a highlight appears there is no
  // first-use shader-compilation stall.
  if (meshes.length === 0) return null;
  return (
    <>
      {meshes.map((m, i) => (
        <Fragment key={i}>
          {createPortal(
            <Outlines
              color={SW1_OUTLINE_COLOR}
              thickness={thickness}
              angle={0}
              toneMapped={false}
              transparent
              opacity={op}
              renderOrder={999}
            />,
            m,
          )}
        </Fragment>
      ))}
    </>
  );
}

// --------------------------------------------------------------------------
// Camera rig: flies to the active shot
// --------------------------------------------------------------------------

// The robot stands on the world vertical axis (x = z = 0). Orbiting the camera
// around THIS axis - rather than lerping its position in a straight line -
// makes it sweep an arc AROUND the robot (e.g. rear <-> front) instead of
// cutting a chord straight THROUGH it.
const ORBIT_X = 0;
const ORBIT_Z = 0;

// The camera reaches a shot in ~1 s (lambda 3.5). Each part animation waits a
// per-anim dwell (seconds since the step became active) before starting, so it
// plays AFTER the camera has essentially come to rest, never blended into the
// move. Timer-based (not a distance threshold) so it is deterministic. Tuned so
// the switch fires a touch sooner and the cable sooner still.
const SW1_DELAY = 1.05; // s before the SW1 slide plays
const CABLE_DELAY = 0.65; // s before the USB cable plugs in
const SCREW_DELAY = 0.45; // s before the screws back out
// The front-face head unit does not pop in/out instantly: it waits a short
// dwell after the shot becomes active, so it fades away only once the camera is
// already moving in (and reappears late when leaving) - in BOTH directions.
const FHEAD_DELAY = 0.45; // s before the front head starts fading in/out
const FHEAD_LAMBDA = 6; // fade speed (higher = quicker cross-fade)
// The shell SLIDE is decoupled from the fade (see FHEAD_WORLD_DIR). It kicks in
// while the LAST screws are still backing out so the lift reads as a tight
// continuation of the unscrew stagger (not a separate beat), then holds while the
// head is "open" and slides home on close.
const FHEAD_SLIDE_DELAY = SCREW_DELAY + SCREW_TOTAL * 0.55; // s before the shell lifts (overlaps the screws)
const FHEAD_SLIDE_LAMBDA = 6; // slide speed (ease-out toward the slid/home pose)

// Seconds for the camera to fly between two shots. The move is a deterministic
// parametric tween (ease-in-out) between the captured start pose and the shot,
// so PREVIOUS retraces the exact same arc as NEXT, just reversed.
const CAM_DURATION = 1.0;

/** smootherstep: symmetric ease-in-out (0 and 1 have zero velocity). */
function smootherstep(x: number): number {
  const t = MathUtils.clamp(x, 0, 1);
  return t * t * t * (t * (t * 6 - 15) + 10);
}

/** Compiles all scene materials (main model + the invisible outline hulls) for a
 * handful of frames after `trigger` flips true. Recompiling is near-free once
 * programs are cached, and spreading it over several frames catches any material
 * (e.g. an <Outlines> hull) that mounts a frame or two late, so the first
 * appearance of any effect never stalls on WebGL shader compilation. */
function Prewarm({ trigger }: { trigger: boolean }) {
  const gl = useThree((s) => s.gl);
  const scene = useThree((s) => s.scene);
  const camera = useThree((s) => s.camera);
  const frames = useRef(0);
  useFrame(() => {
    if (!trigger || frames.current >= 20) return;
    frames.current += 1;
    gl.compile(scene, camera);
  });
  return null;
}

function CameraRig({
  shot,
  active,
  dwellRef,
}: {
  shot: Shot;
  active: boolean;
  dwellRef?: { current: number };
}) {
  const camera = useThree((s) => s.camera) as PerspectiveCamera;
  const desiredPos = useMemo(() => new Vector3(...shot.camPos), [shot]);
  const desiredTarget = useMemo(() => new Vector3(...shot.target), [shot]);
  const lookTarget = useRef(new Vector3(...shot.target));
  const dwell = useRef(0); // seconds since this shot became active
  // Start pose captured when the shot changes, so the tween is a pure function
  // of progress (deterministic) - the key to identical NEXT/PREVIOUS arcs.
  const fromPos = useRef(new Vector3(...shot.camPos));
  const fromTarget = useRef(new Vector3(...shot.target));
  const fromFov = useRef(shot.fov);
  const prog = useRef(1); // 1 = settled on the current shot

  // On a shot change: freeze the current pose as the tween's start and re-arm.
  // MUST be a LAYOUT effect: it has to run synchronously (before paint and before
  // r3f's next render-loop frame) so prog is reset to 0 first. With a passive
  // useEffect, a frame could fire with the new shot while prog was still 1,
  // snapping the camera straight to the target (a "blink" instead of a tween) -
  // and the race resolved differently depending on the navigation direction.
  useLayoutEffect(() => {
    fromPos.current.copy(camera.position);
    fromTarget.current.copy(lookTarget.current);
    fromFov.current = camera.fov;
    prog.current = 0;
    dwell.current = 0;
    if (dwellRef) dwellRef.current = 0;
  }, [shot, camera, dwellRef]);

  useFrame((_, dt) => {
    if (!active) return;
    const step = Math.min(dt, 0.1);
    prog.current = Math.min(prog.current + step / CAM_DURATION, 1);
    const e = smootherstep(prog.current);

    // Interpolate in cylindrical coords about the robot's vertical axis so the
    // camera SWINGS AROUND the robot (never through it). The azimuth delta is
    // the SHORTEST arc between the two FIXED endpoints (not the live position),
    // so a transition and its reverse trace the exact same curve.
    const fa = Math.atan2(fromPos.current.x - ORBIT_X, fromPos.current.z - ORBIT_Z);
    const fr = Math.hypot(fromPos.current.x - ORBIT_X, fromPos.current.z - ORBIT_Z);
    const ta = Math.atan2(desiredPos.x - ORBIT_X, desiredPos.z - ORBIT_Z);
    const tr = Math.hypot(desiredPos.x - ORBIT_X, desiredPos.z - ORBIT_Z);
    const dA = Math.atan2(Math.sin(ta - fa), Math.cos(ta - fa));
    const na = fa + dA * e;
    const nr = fr + (tr - fr) * e;
    const ny = fromPos.current.y + (desiredPos.y - fromPos.current.y) * e;
    camera.position.set(ORBIT_X + Math.sin(na) * nr, ny, ORBIT_Z + Math.cos(na) * nr);

    lookTarget.current.lerpVectors(fromTarget.current, desiredTarget, e);
    camera.lookAt(lookTarget.current);
    camera.fov = fromFov.current + (shot.fov - fromFov.current) * e;
    camera.updateProjectionMatrix();

    // Publish the dwell time (s since this shot became active); each part anim
    // applies its own delay so they start clearly AFTER the camera has stopped.
    dwell.current += step;
    if (dwellRef) dwellRef.current = dwell.current;
  });

  return null;
}

// --------------------------------------------------------------------------
// Animated 3D marker (power button)
// --------------------------------------------------------------------------

const MARK_COLOR = '#FF9500'; // theme primary (see theme.ts ACCENT)
const MARK_R = 0.015; // inner dot radius (m)
const MARK_RING_PERIOD = 1.6; // seconds per ping cycle
const MARK_RING_MAX = 3.0; // ring expands up to this scale

/** A 3D "you are here" marker rendered in the scene: a solid red dot plus two
 * phase-offset rings that expand and fade (a sonar ping). It is billboarded to
 * face the camera and drawn on top (depthTest off) so it's always readable. */
function SceneMarker({ pos }: { pos: Vec3 }) {
  const group = useRef<Group>(null);
  const ring0 = useRef<Mesh>(null);
  const ring1 = useRef<Mesh>(null);
  const t = useRef(0);

  useFrame((state, dt) => {
    const g = group.current;
    if (!g) return;
    g.quaternion.copy(state.camera.quaternion); // billboard toward camera
    t.current += dt;
    const rings = [ring0.current, ring1.current];
    rings.forEach((m, i) => {
      if (!m) return;
      const p = ((t.current / MARK_RING_PERIOD + i * 0.5) % 1 + 1) % 1; // 0..1
      m.scale.setScalar(1 + p * (MARK_RING_MAX - 1));
      (m.material as MeshBasicMaterial).opacity = (1 - p) * 0.85;
    });
  });

  return (
    <group ref={group} position={pos}>
      <mesh renderOrder={999}>
        <circleGeometry args={[MARK_R, 40]} />
        <meshBasicMaterial
          color={MARK_COLOR}
          transparent
          opacity={0.95}
          depthTest={false}
          depthWrite={false}
          toneMapped={false}
        />
      </mesh>
      {[ring0, ring1].map((r, i) => (
        <mesh key={i} ref={r} renderOrder={999}>
          <ringGeometry args={[MARK_R * 0.95, MARK_R * 1.25, 48]} />
          <meshBasicMaterial
            color={MARK_COLOR}
            transparent
            depthTest={false}
            depthWrite={false}
            toneMapped={false}
          />
        </mesh>
      ))}
    </group>
  );
}

// --------------------------------------------------------------------------
// Dev tuning HUD (vite dev only): orbit freely, read camera & clicked points
// --------------------------------------------------------------------------

type DevReadout = { pos: Vec3; target: Vec3 };

function DevCameraReporter({ onRead }: { onRead: (r: DevReadout) => void }) {
  const camera = useThree((s) => s.camera);
  const controls = useThree((s) => s.controls) as { target?: Vector3 } | null;
  const frame = useRef(0);
  useFrame(() => {
    frame.current = (frame.current + 1) % 6;
    if (frame.current !== 0) return;
    const t = controls?.target ?? new Vector3();
    onRead({
      pos: [+camera.position.x.toFixed(2), +camera.position.y.toFixed(2), +camera.position.z.toFixed(2)],
      target: [+t.x.toFixed(2), +t.y.toFixed(2), +t.z.toFixed(2)],
    });
  });
  return null;
}

// --------------------------------------------------------------------------
// Stage
// --------------------------------------------------------------------------

/** How much larger the viz reads than the reserved text-layout slot. The stage
 * is an absolute overlay, so growing it enlarges the robot without shifting any
 * textual content (the reserved slot keeps its original size). */
const STAGE_GROW = 0.15;

/** How far (px) the viz overlay is lifted ABOVE its reserved slot. Raising it
 * opens up a larger gap between the 3D viz and the step title/description below,
 * without touching the (fixed) text layout. */
const STAGE_LIFT = 18;

/** Placeholder slot used before a real media rect exists, so the stage can mount
 * and warm up while hidden. Any sane size works; it's never shown (opacity 0). */
const PRELOAD_RECT = { top: 0, left: 0, width: 320, height: 320 };

export type StageCard = { border: string; background: string; radius: number };

export type ReachyStageProps = {
  shotId: ShotId;
  /** Declared target state of every movable part for this step. The viz eases
   * each part toward it; animation directions follow the state deltas. */
  state: PartState;
  /** Changes when the wizard (re)starts, so parts snap to their state instead of
   * animating a transition into a freshly-shown wizard. */
  snapKey: string;
  visible: boolean;
  /** Absolute rectangle (relative to the positioned parent) of the reserved
   * media slot; the stage is centered on it and grown by STAGE_GROW. */
  rect: { top: number; left: number; width: number; height: number } | null;
  /** Background card look, so the persistent card matches the app theme. */
  card: StageCard;
};

export function ReachyStage({
  shotId,
  state,
  snapKey,
  visible,
  rect,
  card,
}: ReachyStageProps) {
  const shot = SHOTS[shotId];
  const theme = useTheme();
  const [dev, setDev] = useState(false);
  const [readout, setReadout] = useState<DevReadout | null>(null);
  const [picked, setPicked] = useState<Vec3 | null>(null);
  // Loading gate: the model is only "ready" once the glb is decoded AND the
  // antennas have been posed; until then we cover the stage with a spinner.
  const [ready, setReady] = useState(false);
  const handleReady = useCallback(() => setReady(true), []);
  // The model decodes/poses/GPU-warms OFF-SCREEN during the intro, so `ready`
  // (and thus the spinner fade) fires long before the stage is ever shown. That
  // means the FIRST time the viz reveals (first wizard step) the model pops in
  // with a possible 1-frame flicker as the render loop spins up. To mask it, we
  // hold the loading gate a beat longer: it only lifts once the stage has been
  // actually shown, plus a short linger, guaranteeing a settled model beneath.
  const [revealed, setRevealed] = useState(false);
  // The switch meshes, lifted from RobotModel; a drei <Outlines> hull is portaled
  // onto each of them whenever the SW1 step is active.
  const [sw1Meshes, setSw1Meshes] = useState<Object3D[]>([]);
  const handleSw1Meshes = useCallback((objs: Object3D[]) => setSw1Meshes(objs), []);
  // Screw meshes, lifted from RobotModel, outlined by <StepHighlight> on the
  // open-head (screw) step.
  const [screwMeshes, setScrewMeshes] = useState<Object3D[]>([]);
  const handleScrewMeshes = useCallback((objs: Object3D[]) => setScrewMeshes(objs), []);
  // Front head-shell meshes, lifted from RobotModel, outlined by <StepHighlight>
  // on the open/close-head step (the shell is the part being lifted/replaced).
  const [fheadMeshes, setFheadMeshes] = useState<Object3D[]>([]);
  const handleFheadMeshes = useCallback((objs: Object3D[]) => setFheadMeshes(objs), []);
  // USB connector (+ wire) meshes, lifted from UsbCable, outlined on the USB step.
  const [cableMeshes, setCableMeshes] = useState<Object3D[]>([]);
  const handleCableMeshes = useCallback((objs: Object3D[]) => setCableMeshes(objs), []);
  // Shared dwell timer: seconds since the current shot became active (written by
  // CameraRig, read by the part animations, each with its own delay). A ref so
  // it never triggers re-renders.
  const dwellRef = useRef(0);

  useEffect(() => {
    if (!DEV_TUNING) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'd' && (e.metaKey || e.altKey)) setDev((v) => !v);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // The stage stays MOUNTED from app launch so the glb decodes, poses and
  // GPU-warms (Preload) while it's still hidden - it's already good the first
  // time it appears. Before any real media slot exists (e.g. the intro screen)
  // we render it off in a corner, fully hidden. It only "shows" once there's a
  // real slot AND the wizard wants it visible.
  const shown = visible && rect != null;
  const active = shown && !dev;
  const slot = rect ?? PRELOAD_RECT;

  // Once the model is ready AND has actually been shown, keep the spinner up for
  // ~1s more before lifting it - long enough for the first real frames to settle
  // so the reveal is flicker-free. Sticky: later reveals stay uncovered.
  useEffect(() => {
    if (!ready || !shown || revealed) return;
    const id = setTimeout(() => setRevealed(true), 1000);
    return () => clearTimeout(id);
  }, [ready, shown, revealed]);

  // Grow centered on the reserved slot: text layout stays untouched.
  const grownW = slot.width * (1 + STAGE_GROW);
  const grownH = slot.height * (1 + STAGE_GROW);
  const grownRect = {
    width: grownW,
    height: grownH,
    // Center on the slot, then lift the whole overlay up so it clears the title.
    top: slot.top - (grownH - slot.height) / 2 - STAGE_LIFT,
    left: slot.left - (grownW - slot.width) / 2,
  };

  return (
    <StageWrapper rect={grownRect} visible={shown} card={card}>
      <Canvas
        frameloop={shown ? 'always' : 'demand'}
        dpr={[1, 2]}
        gl={{ alpha: true, antialias: true, preserveDrawingBuffer: false }}
        camera={{ position: SHOTS.full.camPos, fov: SHOTS.full.fov, near: 0.01, far: 50 }}
      >
        <ambientLight intensity={0.5} />
        <hemisphereLight intensity={0.45} groundColor="#3a3a3a" />
        <directionalLight position={[2, 4, 3]} intensity={1.25} />
        <directionalLight position={[-3, 2, -2]} intensity={0.5} />
        {/* Runtime-baked cubemap (no CDN): gives the plastic/metal materials
         * real reflections so the model reads as shaded, not flat. */}
        <Environment resolution={64} frames={1}>
          <Lightformer intensity={1.6} position={[0, 4, 3]} scale={[6, 6, 1]} />
          <Lightformer intensity={0.9} position={[-4, 1, -3]} scale={[5, 5, 1]} />
          <Lightformer intensity={0.6} position={[4, 0, 2]} scale={[4, 4, 1]} />
        </Environment>

        <Suspense fallback={null}>
          <RobotModel
            hiddenParts={shot.hiddenParts}
            headOpen={state.headOpen}
            screwsOut={state.screwsOut}
            sw1Download={state.sw1Download}
            snapKey={snapKey}
            dwellRef={dwellRef}
            onPick={dev ? setPicked : undefined}
            onReady={handleReady}
            onSw1Meshes={handleSw1Meshes}
            onScrewMeshes={handleScrewMeshes}
            onFheadMeshes={handleFheadMeshes}
          />
          {/* Render EVERY object once (incl. off-screen / occluded ones) to force
           * all geometry buffers, textures and shaders onto the GPU up front, so
           * no later step stalls the first time it reveals the inner board. */}
          <Preload all />
        </Suspense>

        <UsbCable
          plugged={state.cablePlugged}
          snapKey={snapKey}
          dwellRef={dwellRef}
          onMeshes={handleCableMeshes}
        />

        {shot.marker3d && <SceneMarker pos={shot.marker3d} />}

        {/* Force-compile every material once the model + outline hulls exist, so
         * no step ever stalls on first-use shader compilation. */}
        <Prewarm trigger={ready && sw1Meshes.length > 0 && screwMeshes.length > 0} />

        <CameraRig shot={shot} active={active} dwellRef={dwellRef} />

        {/* Transient primary-colored outline highlights (crisp inverted-hull via
         * drei's <Outlines>), faded in a bit after the step starts and out
         * before it ends - on the switch (SW1 step) and on the screws (open-head
         * step). See <StepHighlight>. */}
        <StepHighlight
          meshes={sw1Meshes}
          active={active && !!shot.highlightSw1}
          dwellRef={dwellRef}
          thickness={SW1_OUTLINE_PX}
        />
        <StepHighlight
          meshes={screwMeshes}
          active={active && !!shot.highlightScrews}
          dwellRef={dwellRef}
          thickness={SCREW_OUTLINE_PX}
        />
        <StepHighlight
          meshes={fheadMeshes}
          active={active && !!shot.highlightHead}
          dwellRef={dwellRef}
          thickness={SW1_OUTLINE_PX}
        />
        <StepHighlight
          meshes={cableMeshes}
          active={active && !!shot.highlightCable}
          dwellRef={dwellRef}
          thickness={SW1_OUTLINE_PX}
        />

        {dev && (
          <>
            <OrbitControls makeDefault target={shot.target} />
            <DevCameraReporter onRead={setReadout} />
          </>
        )}
      </Canvas>

      {dev && (
        <div
          style={{
            position: 'absolute',
            left: 6,
            bottom: 6,
            font: '11px/1.4 monospace',
            color: '#fff',
            background: 'rgba(0,0,0,0.7)',
            padding: '6px 8px',
            borderRadius: 6,
            pointerEvents: 'none',
            whiteSpace: 'pre',
          }}
        >
          {`shot: ${shotId}\ncamPos: [${readout?.pos ?? '?'}]\ntarget: [${readout?.target ?? '?'}]\npick:   [${picked ?? '-'}]`}
        </div>
      )}

      {/* Loading gate: cover the stage until the model is decoded and the
          antennas are posed, so the user never sees a blank / half-rigged frame. */}
      <div
        style={{
          position: 'absolute',
          inset: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          opacity: ready && revealed ? 0 : 1,
          pointerEvents: 'none',
          // Opaque surface fill so the half-decoded / un-posed model is fully
          // hidden behind the spinner (the canvas is alpha-transparent).
          background: theme.palette.background.default,
          borderRadius: card.radius,
          // Smoother, slower fade-out once we finally lift the gate (the ~1s
          // hold before `revealed` already provides the linger).
          transition: 'opacity 0.5s ease',
          zIndex: 3,
        }}
      >
        <CircularProgress size={28} thickness={4} sx={{ color: 'text.disabled' }} />
      </div>
    </StageWrapper>
  );
}

/** Absolutely-positioned wrapper that overlays the media frame. Kept mounted at
 * all times (the WebGL context and loaded model persist); only visibility and
 * the render loop toggle so the viz stays "constant" across steps. */
function StageWrapper({
  rect,
  visible,
  card,
  children,
}: {
  rect: { top: number; left: number; width: number; height: number };
  visible: boolean;
  card: StageCard;
  children: ReactNode;
}) {
  return (
    <div
      style={{
        position: 'absolute',
        top: rect.top,
        left: rect.left,
        width: rect.width,
        height: rect.height,
        borderRadius: card.radius,
        border: card.border,
        background: card.background,
        overflow: 'hidden',
        opacity: visible ? 1 : 0,
        pointerEvents: visible ? 'auto' : 'none',
        // Fade in with a short delay so the viz reveals in step with the text
        // body below (which waits for the previous screen to exit); fade out
        // immediately when leaving a viz screen.
        transition: 'opacity 0.28s ease',
        transitionDelay: visible ? '0.16s' : '0s',
        zIndex: 2,
      }}
    >
      {children}
    </div>
  );
}
