/**
 * Camera "shots" and per-step part STATE for the guided flasher viz.
 *
 * The persistent 3D stage (see ReachyStage.tsx) never remounts; each wizard
 * step just picks a shot and the camera flies to it. Coordinates are in the
 * exported glb's frame (Y-up, metres) - derived from the Blender model with the
 * glTF convention (x, y, z)_gltf = (x, z, -y)_blender. They are intentionally
 * easy to tweak: enable dev mode in the stage to read live camera/target/point
 * values and paste refined numbers here.
 *
 * DECLARATIVE MODEL: rather than telling the viz *how* to animate (open this,
 * unscrew that, reverse on the way back...), each step simply declares the
 * TARGET STATE of every movable part. The viz eases each part toward its target
 * after the camera settles, so the direction of every animation (screw in/out,
 * plug/unplug, open/close, toggle) falls out automatically from how the state
 * changes between consecutive steps. No per-part "reverse"/"hold" flags.
 */

export type Vec3 = [number, number, number];

export type Shot = {
  /** Camera position. */
  camPos: Vec3;
  /** Point the camera looks at. */
  target: Vec3;
  fov: number;
  /** Case-insensitive name substrings of meshes to hide for this shot (e.g.
   * lift the head shell so the inner board is visible). */
  hiddenParts?: string[];
  /** Draw the transient primary outline highlight on the SW1 switch this shot. */
  highlightSw1?: boolean;
  /** Draw the transient primary outline highlight on the head screws this shot. */
  highlightScrews?: boolean;
  /** Draw the transient primary outline highlight on the front head shell this shot. */
  highlightHead?: boolean;
  /** Draw the transient primary outline highlight on the USB connector + wire. */
  highlightCable?: boolean;
  /** World-space position (glb frame) of an animated 3D marker rendered in the
   * scene. Used only for the power button on the power on/off steps. */
  marker3d?: Vec3;
};

export type ShotId = 'full' | 'head' | 'sw1' | 'usb';

// The robot's face (eyes) points toward glTF -Z (Blender +Y). So front views
// sit at negative Z, and the head screws - which live on the BACK of the head
// (Blender -Y) - are shown from positive Z (rear).

export const SHOTS: Record<ShotId, Shot> = {
  // Power on/off steps: the physical power switch is on the BACK of the base,
  // right at the bottom. So we look from BEHIND (+Z) and low, framing the whole
  // robot without clipping its base, with an animated 3D marker on the button.
  full: {
    camPos: [0.18, 0.1, 1.3],
    target: [0, 0.3, 0.06],
    fov: 36,
    marker3d: [0.048, 0.015, 0.122],
  },
  // Head from the rear-above, where the 4 shell screws are. The 4 screws animate
  // (out on disassembly, back in on reassembly) and are highlighted.
  head: {
    camPos: [0, 0.95, 0.72],
    target: [0, 0.52, 0.03],
    fov: 36,
    highlightScrews: true,
    highlightHead: true,
  },
  // Looking into the head from the front-above (front shell + eyes lifted) at
  // the inner board around SW1.
  // The two board steps keep the SAME orientation, distance and fov: the only
  // difference is a lateral PAN so the view is centred on the component of
  // interest. SW1 is centred on the slide switch (to the left, lower x).
  sw1: {
    camPos: [0.103, 0.549, -0.115],
    target: [0.103, 0.467, -0.038],
    fov: 30,
    highlightSw1: true,
  },
  // USB centred on the CM4 USB-C port (to the right, higher x) - same pose/zoom
  // as sw1, just panned over.
  usb: {
    camPos: [0.134, 0.548, -0.116],
    target: [0.134, 0.466, -0.039],
    fov: 30,
    highlightCable: true,
  },
};

/**
 * Physical state of every movable part. The viz holds each part at these values
 * and animates the transitions between steps.
 */
export type PartState = {
  /** Front-face head unit removed (board exposed). */
  headOpen: boolean;
  /** 4 head-shell screws backed out of their holes. */
  screwsOut: boolean;
  /** SW1 slide switch in DOWNLOAD (vs DEBUG). */
  sw1Download: boolean;
  /** USB-C cable seated in the CM4 port. */
  cablePlugged: boolean;
};

export type StepShot = PartState & {
  /** Which camera shot this step uses. */
  shot: ShotId;
};

/** Fully assembled, powered-off robot - the resting state outside the wizards. */
export const ASSEMBLED: PartState = {
  headOpen: false,
  screwsOut: false,
  sw1Download: false,
  cablePlugged: false,
};

/**
 * CONNECT wizard (disassembly, before the flash): power off, open the head,
 * switch to DOWNLOAD, plug the USB cable, power on. Each step leaves the parts
 * it changed in their new state, so later steps just keep them there.
 */
export const CONNECT_STEPS_VIZ: StepShot[] = [
  // 0 - power off: everything still assembled.
  { shot: 'full', headOpen: false, screwsOut: false, sw1Download: false, cablePlugged: false },
  // 1 - open the head: the 4 screws back out (head shell still shown from rear).
  { shot: 'head', headOpen: false, screwsOut: true, sw1Download: false, cablePlugged: false },
  // 2 - switch to DOWNLOAD: front head lifted, board exposed, switch slides over.
  { shot: 'sw1', headOpen: true, screwsOut: true, sw1Download: true, cablePlugged: false },
  // 3 - plug the USB cable in.
  { shot: 'usb', headOpen: true, screwsOut: true, sw1Download: true, cablePlugged: true },
  // 4 - power on (download mode): still open/plugged.
  { shot: 'full', headOpen: true, screwsOut: true, sw1Download: true, cablePlugged: true },
];

/**
 * RESTART wizard (reassembly, after the flash): power off, switch back to
 * NORMAL, unplug the cable, close the head, power on. It starts where CONNECT
 * left off (open, plugged, DOWNLOAD) and reverses each action - the directions
 * come for free from the state deltas.
 */
export const DONE_STEPS_VIZ: StepShot[] = [
  // 0 - power off: still open/plugged/DOWNLOAD from before the flash.
  { shot: 'full', headOpen: true, screwsOut: true, sw1Download: true, cablePlugged: true },
  // 1 - switch back to DEBUG.
  { shot: 'sw1', headOpen: true, screwsOut: true, sw1Download: false, cablePlugged: true },
  // 2 - unplug the USB cable.
  { shot: 'usb', headOpen: true, screwsOut: true, sw1Download: false, cablePlugged: false },
  // 3 - close the head: front head back on AND the 4 screws screw back in.
  { shot: 'head', headOpen: false, screwsOut: false, sw1Download: false, cablePlugged: false },
  // 4 - power on: fully reassembled.
  { shot: 'full', headOpen: false, screwsOut: false, sw1Download: false, cablePlugged: false },
];
