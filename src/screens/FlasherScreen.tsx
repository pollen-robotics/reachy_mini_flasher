import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import ButtonBase from '@mui/material/ButtonBase';
import CircularProgress from '@mui/material/CircularProgress';
import Dialog from '@mui/material/Dialog';
import DialogActions from '@mui/material/DialogActions';
import DialogContent from '@mui/material/DialogContent';
import DialogContentText from '@mui/material/DialogContentText';
import DialogTitle from '@mui/material/DialogTitle';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { alpha, useTheme } from '@mui/material/styles';
import type { SxProps, Theme } from '@mui/material/styles';
import type { SvgIconComponent } from '@mui/icons-material';
import AccessTimeRounded from '@mui/icons-material/AccessTimeRounded';
import CheckCircleRounded from '@mui/icons-material/CheckCircleRounded';
import ChevronLeftRounded from '@mui/icons-material/ChevronLeftRounded';
import ChevronRightRounded from '@mui/icons-material/ChevronRightRounded';
import HandymanOutlined from '@mui/icons-material/HandymanOutlined';
import PowerSettingsNewOutlined from '@mui/icons-material/PowerSettingsNewOutlined';
import ToggleOffOutlined from '@mui/icons-material/ToggleOffOutlined';
import ToggleOnOutlined from '@mui/icons-material/ToggleOnOutlined';
import UsbOutlined from '@mui/icons-material/UsbOutlined';
import WarningAmberRounded from '@mui/icons-material/WarningAmberRounded';

import reachyImg from '@/assets/reachy-builder.svg';
import reachyAvatarImg from '@/assets/reachy.svg';
import { ReachyStage } from '@/components/reachy-viz/ReachyStage';
import {
  ASSEMBLED,
  CONNECT_STEPS_VIZ,
  DONE_STEPS_VIZ,
  type StepShot,
} from '@/components/reachy-viz/shots';
import { FONT_WEIGHT } from '@/design/tokens';
import {
  detectReachy,
  flashReachy,
  onFlashProgress,
  openUrl,
  prefetchImage,
  prepareReachy,
  type FlashProgress,
  type ReachyDevice,
} from '@/lib/api';

/** Public troubleshooting docs (same target as the mobile app). */
const TROUBLESHOOTING_URL = 'https://huggingface.co/docs/reachy_mini/troubleshooting';

type Status =
  | 'intro'
  | 'connect'
  | 'found'
  | 'ready'
  | 'flashing'
  | 'flashed'
  | 'done'
  | 'error';

type StepItem = {
  label: string;
  Icon: SvgIconComponent;
  desc?: ReactNode;
  /** Optional highlighted callout rendered as a small tinted card below the desc. */
  note?: ReactNode;
};

/** Shared outlined-tag look (grey, relatively contrasted), used by inline
 * keyword tags and standalone pills like the version badge. */
const TAG_BORDER = (t: Theme) => `1px solid ${alpha(t.palette.text.primary, 0.3)}`;

/** Inline keyword tag: highlights the important term of a step as a soft pill.
 * `boxDecorationBreak: clone` keeps the pill clean if it wraps across lines. */
function B({ children }: { children: ReactNode }) {
  return (
    <Box
      component="span"
      sx={{
        fontWeight: 600,
        color: 'text.secondary',
        border: TAG_BORDER,
        borderRadius: '6px',
        px: 0.6,
        py: 0.15,
        mx: 0.2,
        WebkitBoxDecorationBreak: 'clone',
        boxDecorationBreak: 'clone',
      }}
    >
      {children}
    </Box>
  );
}

/** Inline bold emphasis for secondary important words (no pill). */
function S({ children }: { children: ReactNode }) {
  return (
    <Box component="span" sx={{ fontWeight: 700, color: 'text.primary' }}>
      {children}
    </Box>
  );
}

const CONNECT_STEPS: StepItem[] = [
  {
    label: 'Power off the robot',
    Icon: PowerSettingsNewOutlined,
    desc: (
      <>
        Make sure your Reachy is <S>switched off</S> before you start.
      </>
    ),  },
  {
    label: "Open the robot's head",
    Icon: HandymanOutlined,
    desc: (
      <>
        Use the <B>screwdriver</B> to <S>remove the 4 head screws</S>, then{' '}
        <S>lift the top shell</S> to reach the board.
      </>
    ),  },
  {
    label: 'Switch to DOWNLOAD',
    Icon: ToggleOnOutlined,
    desc: (
      <>
        Find the small <B>SW1</B> switch and <S>push it toward</S> the <B>DOWNLOAD</B> label.
      </>
    ),  },
  {
    label: 'Plug in the USB cable',
    Icon: UsbOutlined,
    desc: (
      <>
        <S>Connect the cable</S> to the <B>CM4 USB port</B> inside the head, and the{' '}
        <S>other end</S> to your <B>computer</B>.
      </>
    ),  },
  {
    label: 'Power on the robot',
    Icon: PowerSettingsNewOutlined,
    desc: (
      <>
        <S>Switch it back on</S> - after a few seconds you should hear the <B>fan spinning</B>. It
        boots in <B>download mode</B>, ready to flash.
      </>
    ),  },
];

const DONE_STEPS: StepItem[] = [
  {
    label: 'Power off the robot',
    Icon: PowerSettingsNewOutlined,
    desc: (
      <>
        <S>Turn your Reachy off</S> now that the new system has been written.
      </>
    ),  },
  {
    label: 'Switch to DEBUG',
    Icon: ToggleOffOutlined,
    desc: (
      <>
        <S>Move</S> the <B>SW1</B> switch back from <B>DOWNLOAD</B> to its <B>DEBUG</B> position.
      </>
    ),  },
  {
    label: 'Unplug the USB cable',
    Icon: UsbOutlined,
    desc: (
      <>
        <S>Disconnect the USB cable</S> from the <B>CM4 port</B> inside the head and from your{' '}
        <B>computer</B>.
      </>
    ),  },
  {
    label: 'Close the head',
    Icon: HandymanOutlined,
    desc: (
      <>
        <S>Put the top shell back</S> and <S>screw the 4 head screws back in</S> with the{' '}
        <B>screwdriver</B>.
      </>
    ),  },
  {
    label: 'Power on the robot',
    Icon: PowerSettingsNewOutlined,
    desc: (
      <>
        <S>Switch it back on</S> - the <B>fan</B> should spin and it boots the fresh{' '}
        <B>ReachyMiniOS</B>.
      </>
    ),
    note: (
      <>
        <S>Wait a few minutes</S> before starting the <B>Bluetooth</B> setup.
      </>
    ),
  },
];

/** Number of guided hardware instructions in the Connect wizard. */
const CONNECT_N = CONNECT_STEPS.length;

/** Number of guided restart instructions after a successful flash. */
const DONE_N = DONE_STEPS.length;

/**
 * Single, unified progress across the WHOLE journey - there is only ever ONE
 * stepper (the thin top bar). The Connect and Done wizards have no counter of
 * their own; each Next just pushes this bar forward. Ordered positions:
 *   intro -> [connect 0..N] -> found -> install -> flashing -> flashed -> [restart 0..DONE_N-1] -> finished
 * During flashing the bar eases within its own segment using the write %.
 */
function journeyValue(
  status: Status,
  connectStep: number,
  doneStep: number,
  flashPct: number | null,
): number {
  const doneBase = CONNECT_N + 6;
  const denom = doneBase + DONE_N; // index of the final ("finished") position
  let idx: number;
  switch (status) {
    case 'intro':
      idx = 0;
      break;
    case 'connect':
      idx = 1 + Math.min(connectStep, CONNECT_N); // 1..N+1 (N = waiting)
      break;
    case 'found':
      idx = CONNECT_N + 2;
      break;
    case 'ready':
      idx = CONNECT_N + 3;
      break;
    case 'flashing':
    case 'error':
      idx = CONNECT_N + 4 + (status === 'flashing' && flashPct != null ? flashPct / 100 : 0);
      break;
    case 'flashed':
      idx = CONNECT_N + 5;
      break;
    case 'done':
      idx = doneBase + Math.min(doneStep, DONE_N);
      break;
  }
  return (idx / denom) * 100;
}

/** Reserved height for the centered body zone so swapping between states doesn't
 * reflow what sits below (footer stays put, nothing jitters). Sized for the
 * wizard's media block so every screen keeps a consistent visual weight. */
const BODY_MIN_H = 320;

/** Fixed height of the visual slot that sits atop every screen's composition
 * (step photo/icon for the wizard, the Reachy mark otherwise). Constant across
 * screens so the vertical rhythm never shifts. */
const VISUAL_H = 190;

/** Every screen's top visual lives in this ONE frame (photo, icon or the Reachy
 * mark), so the composition is identical from screen to screen. */
// Match the native 3:2 aspect ratio of the step photos (2673x1808) so they
// fill the frame edge-to-edge without letterboxing or cropping. Sized to fit
// within VISUAL_H.
const MEDIA_W = 280;
const MEDIA_H = 189;

/** Fixed height of a row in the "Select your Reachy" list, so the searching
 * placeholder and the detected-device row are exactly the same height (no jump
 * when a robot appears). */
const SELECT_ROW_H = 66;

/** A single button in the bottom action bar. */
type BarAction = { label: string; onClick: () => void; disabled?: boolean };

/** Large, prominent title + supporting description, sized up for the wide window. */
const TITLE_SX = {
  fontSize: '1.75rem',
  fontWeight: FONT_WEIGHT.bold,
  letterSpacing: '-0.5px',
  lineHeight: 1.15,
} as const;

const DESC_SX = {
  fontSize: '1.0625rem',
  color: 'text.secondary',
  lineHeight: 1.5,
  maxWidth: 340,
  marginInline: 'auto',
} as const;

/** Shared layout for every screen body: centered content, full width. */
const BODY_STACK_SX = {
  alignItems: 'center',
  textAlign: 'center',
  width: '100%',
} as const;

export function FlasherScreen() {
  const theme = useTheme();
  const [status, setStatus] = useState<Status>('intro');
  const [connectStep, setConnectStep] = useState(0);
  const [doneStep, setDoneStep] = useState(0);
  const [device, setDevice] = useState<ReachyDevice | null>(null);
  // Whether the user has picked the detected robot in the list. Next stays
  // disabled until this is true.
  const [selected, setSelected] = useState(false);
  // Confirmation modal before the (destructive) flash.
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [progress, setProgress] = useState<FlashProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Background OS image prefetch (non-blocking).
  const [imageReady, setImageReady] = useState(false);
  const [imageProgress, setImageProgress] = useState<FlashProgress | null>(null);
  const [imageError, setImageError] = useState<string | null>(null);
  const [osVersion, setOsVersion] = useState<string | null>(null);

  // rpiboot preparation (download mode -> expose eMMC).
  const [preparing, setPreparing] = useState(false);
  const [prepareError, setPrepareError] = useState<string | null>(null);
  const prepareStartedRef = useRef(false);

  // After a while without any device, hint that the board might be dead.
  const [waitTimedOut, setWaitTimedOut] = useState(false);

  const statusRef = useRef(status);
  statusRef.current = status;
  const connectStepRef = useRef(connectStep);
  connectStepRef.current = connectStep;

  // Persistent 3D stage plumbing. The stage is rendered ONCE at the screen root
  // and absolutely positioned over the wizard's media frame; we measure that
  // frame's rectangle so the overlay lines up exactly and never remounts.
  const rootRef = useRef<HTMLDivElement>(null);
  const mediaNodeRef = useRef<HTMLElement | null>(null);
  const [mediaRect, setMediaRect] = useState<{
    top: number;
    left: number;
    width: number;
    height: number;
  } | null>(null);

  const measureMedia = useCallback(() => {
    const el = mediaNodeRef.current;
    const root = rootRef.current;
    if (!el || !root) return;
    const r = el.getBoundingClientRect();
    const rr = root.getBoundingClientRect();
    setMediaRect({ top: r.top - rr.top, left: r.left - rr.left, width: r.width, height: r.height });
  }, []);

  const registerMedia = useCallback(
    (el: HTMLDivElement | null) => {
      mediaNodeRef.current = el;
      if (el) requestAnimationFrame(measureMedia);
    },
    [measureMedia],
  );

  useEffect(() => {
    const on = () => measureMedia();
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  }, [measureMedia]);

  const startPrefetch = useCallback(() => {
    setImageError(null);
    prefetchImage()
      .then(() => setImageReady(true))
      .catch((e) => setImageError(String(e)));
  }, []);

  // Kick off the download immediately, in the background.
  useEffect(() => {
    startPrefetch();
  }, [startPrefetch]);

  // Poll for a connected Reachy from the very first frame (independent of the
  // download): the user can prepare the robot while the image is fetched.
  useEffect(() => {
    let active = true;
    const tick = async () => {
      // Only look for the robot once the user has walked the wizard and reached
      // the waiting screen - not while they're still reading the instructions.
      if (statusRef.current !== 'connect' || connectStepRef.current < CONNECT_N) return;
      try {
        const found = await detectReachy();
        if (!active) return;
        // Robot no longer visible (unplugged, or dropped out of download mode
        // mid-prep): re-arm so a fresh plug-in triggers one new attempt, and
        // clear the "preparing" state so the UI doesn't hang on it forever.
        if (!found) {
          prepareStartedRef.current = false;
          setPreparing(false);
          return;
        }
        if (found.mode === 'download') {
          if (!prepareStartedRef.current) {
            prepareStartedRef.current = true;
            setPreparing(true);
            prepareReachy().catch((e) => {
              if (!active) return;
              // Do NOT re-arm here: leaving the guard set prevents re-prompting
              // for the admin password every poll (~1.5s) while the robot stays
              // in download mode. The user retries via the "Try again" button,
              // or by re-plugging the robot (handled by the `!found` branch).
              setPreparing(false);
              setPrepareError(String(e));
            });
          }
          return;
        }
        // Surface the detected Reachy and auto-select it (there's only ever one)
        // so the user just has to confirm with Next. Polling stops once we leave
        // the 'connect' state.
        setDevice(found);
        setSelected(true);
        setStatus('found');
      } catch {
        /* keep waiting */
      }
    };
    void tick();
    const id = setInterval(tick, 1500);
    return () => {
      active = false;
      clearInterval(id);
    };
  }, []);

  // If we've been on the waiting screen for a while with nothing detected,
  // surface a "board might be dead" hint + troubleshooting link.
  const waiting = status === 'connect' && connectStep >= CONNECT_N && !device && !preparing;
  useEffect(() => {
    if (!waiting) {
      setWaitTimedOut(false);
      return;
    }
    const id = setTimeout(() => setWaitTimedOut(true), 25000);
    return () => clearTimeout(id);
  }, [waiting]);

  // Route progress: downloading -> chip + version, otherwise -> flash.
  useEffect(() => {
    const unlisten = onFlashProgress((p) => {
      if (p.phase === 'downloading') {
        setImageProgress(p);
        if (p.version) setOsVersion(p.version);
      } else {
        setProgress(p);
        if (p.phase === 'done') setStatus('flashed');
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const handleFlash = useCallback(async () => {
    setError(null);
    setProgress(null);
    setStatus('flashing');
    try {
      await flashReachy();
      setStatus('flashed');
    } catch (e) {
      setError(String(e));
      setStatus('error');
    }
  }, []);

  // "Flash another": restart the flow but skip the intro - go straight to the
  // first connect instruction.
  const handleReset = useCallback(() => {
    setStatus('connect');
    setConnectStep(0);
    setDoneStep(0);
    setDevice(null);
    setSelected(false);
    setProgress(null);
    setError(null);
    setPreparing(false);
    setPrepareError(null);
    prepareStartedRef.current = false;
  }, []);

  // Leave the intro and enter the connect wizard at its first step.
  const startFlow = useCallback(() => {
    setConnectStep(0);
    setStatus('connect');
  }, []);

  // Connect wizard navigation (shared by live + dev). Clamped to [0, CONNECT_N];
  // reaching CONNECT_N is the "waiting for the robot" screen.
  const connectNext = useCallback(() => {
    setConnectStep((s) => Math.min(s + 1, CONNECT_N));
  }, []);
  const connectBack = useCallback(() => {
    setPrepareError(null);
    setConnectStep((s) => Math.max(s - 1, 0));
  }, []);

  // Leave the "flash complete" recap and enter the guided restart wizard.
  const startRestart = useCallback(() => {
    setDoneStep(0);
    setStatus('done');
  }, []);

  // Restart wizard navigation (after a successful flash). Clamped to [0, DONE_N];
  // reaching DONE_N is the final "all set" screen.
  const doneNext = useCallback(() => setDoneStep((s) => Math.min(s + 1, DONE_N)), []);
  const doneBack = useCallback(() => setDoneStep((s) => Math.max(s - 1, 0)), []);

  const retryPrepare = useCallback(() => {
    setPrepareError(null);
    setPreparing(false);
    prepareStartedRef.current = false;
  }, []);

  // User picked the detected Reachy in the list (toggles the selection).
  const toggleSelect = useCallback(() => setSelected((s) => !s), []);

  // User confirmed the selected Reachy: continue to the flash step.
  const handleSelect = useCallback(() => setStatus('ready'), []);

  const downloadPct = (() => {
    const t = imageProgress?.total ?? 0;
    const w = imageProgress?.written ?? 0;
    return t > 0 ? Math.min(100, Math.round((w / t) * 100)) : null;
  })();

  const effStatus = status;
  const effConnectStep = connectStep;
  const effDoneStep = doneStep;
  const effDevice = device;
  const effVersion = osVersion;
  const effImageReady = imageReady;
  const effProgress = progress;
  const effPreparing = preparing;
  const effPrepareError = prepareError;
  const effError = error ?? 'Something went wrong';
  const effDownloading = !imageReady && !imageError;
  const effDownloadPct = downloadPct;
  const effImageError = imageError;

  const onSelect = handleSelect;
  const onFlash = handleFlash;
  const onReset = handleReset;
  const onNext = connectNext;
  const onBack = connectBack;
  const onDoneNext = doneNext;
  const onDoneBack = doneBack;

  // Cross-status Back targets.
  const backToLastInstruction = () => {
    setSelected(false);
    setConnectStep(CONNECT_N - 1);
    setStatus('connect');
  };
  const backToFound = () => setStatus(device ? 'found' : 'connect');
  const backToReady = () => setStatus('ready');

  // The single bottom action bar: Back on the left, primary on the right. Config
  // is centralized here so button position/label stay consistent across screens.
  let backAction: BarAction | null = null;
  let primaryAction: BarAction | null = null;
  if (effStatus === 'intro') {
    primaryAction = { label: 'Get started', onClick: startFlow };
  } else if (effStatus === 'connect') {
    if (effConnectStep >= CONNECT_N) {
      backAction = { label: 'Back', onClick: onBack };
      // Still searching: Next is present but disabled until a robot is picked.
      primaryAction = effPrepareError
        ? { label: 'Try again', onClick: retryPrepare }
        : { label: 'Next', onClick: onSelect, disabled: true };
    } else {
      // First step steps back to the intro (continuous back navigation).
      backAction = {
        label: 'Back',
        onClick: effConnectStep === 0 ? () => setStatus('intro') : onBack,
      };
      primaryAction = {
        label: effConnectStep === CONNECT_N - 1 ? "I'm ready" : 'Next',
        onClick: onNext,
      };
    }
  } else if (effStatus === 'found') {
    backAction = { label: 'Back', onClick: backToLastInstruction };
    // Next only becomes available once the user selects the detected Reachy.
    primaryAction = { label: 'Next', onClick: onSelect, disabled: !(selected && effDevice) };
  } else if (effStatus === 'ready') {
    backAction = { label: 'Back', onClick: backToFound };
    primaryAction = {
      label: 'Flash Reachy',
      onClick: () => setConfirmOpen(true),
      disabled: !effImageReady,
    };
  } else if (effStatus === 'flashed') {
    // Back returns to the install screen so you can re-flash if needed.
    backAction = { label: 'Back', onClick: backToReady };
    primaryAction = { label: 'Next', onClick: startRestart };
  } else if (effStatus === 'done') {
    if (effDoneStep >= DONE_N) {
      // Final recap: no Back - the journey is done, only "Flash another".
      primaryAction = { label: 'Flash another', onClick: onReset };
    } else {
      // Continuous back: from the first restart step, step back to the recap.
      backAction = {
        label: 'Back',
        onClick: effDoneStep === 0 ? () => setStatus('flashed') : onDoneBack,
      };
      primaryAction = {
        label: effDoneStep === DONE_N - 1 ? 'Finish' : 'Next',
        onClick: onDoneNext,
      };
    }
  } else if (effStatus === 'error') {
    backAction = { label: 'Back', onClick: backToReady };
    primaryAction = { label: 'Try again', onClick: onFlash };
  }
  // 'flashing' intentionally leaves both null: no action while writing.

  const flashPct = (() => {
    const t = effProgress?.total ?? 0;
    const w = effProgress?.written ?? 0;
    return t > 0 ? Math.min(100, Math.round((w / t) * 100)) : null;
  })();
  const barValue = journeyValue(effStatus, effConnectStep, effDoneStep, flashPct);

  // Current step name, shown small & light in the top-right corner (there are
  // too many steps for a numeric counter to be meaningful).
  const stepLabel = ((): string => {
    switch (effStatus) {
      case 'intro':
        return 'Overview';
      case 'connect':
        return effConnectStep < CONNECT_N ? CONNECT_STEPS[effConnectStep].label : 'Find your Reachy';
      case 'found':
        return 'Find your Reachy';
      case 'ready':
        return 'Install ReachyMiniOS';
      case 'flashing':
        return 'Writing image';
      case 'flashed':
        return 'Flash complete';
      case 'done':
        return effDoneStep < DONE_N ? DONE_STEPS[effDoneStep].label : 'All set';
      case 'error':
        return 'Error';
    }
  })();

  // Identity of the current screen, used to key the enter/exit content animation.
  const viewKey =
    (effStatus === 'connect' && effConnectStep >= CONNECT_N) || effStatus === 'found'
      ? 'select'
      : effStatus === 'connect'
        ? `connect-${effConnectStep}`
        : effStatus === 'done'
          ? `done-${effDoneStep}`
          : effStatus;

  // The persistent viz is only on-screen during the guided hardware wizards
  // (connect + restart). Everywhere else it stays mounted but hidden.
  const vizVisible =
    (effStatus === 'connect' && effConnectStep < CONNECT_N) ||
    (effStatus === 'done' && effDoneStep < DONE_N);
  const isConnectStep = effStatus === 'connect' && effConnectStep < CONNECT_N;
  const isDoneStep = effStatus === 'done' && effDoneStep < DONE_N;
  // Declarative per-step spec (shot + target part state). The viz derives every
  // animation from how this state changes step to step (see shots.ts).
  const vizStep: StepShot = isDoneStep
    ? DONE_STEPS_VIZ[effDoneStep]
    : isConnectStep
      ? CONNECT_STEPS_VIZ[effConnectStep]
      : { shot: 'full', ...ASSEMBLED };
  const { shot: shotId, ...vizState } = vizStep;
  // Snap (vs animate) the parts when the wizard the viz belongs to changes, so a
  // freshly-shown wizard opens already posed instead of animating on step 0.
  const vizSnapKey = isDoneStep ? 'done' : isConnectStep ? 'connect' : 'idle';

  return (
    <Box
      ref={rootRef}
      sx={{
        height: '100vh',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        px: 2.5,
        pt: '46px',
        pb: 1,
        overflowX: 'hidden',
        position: 'relative',
      }}
    >
      <StepBar value={barValue} />

      <Typography
        sx={{
          position: 'absolute',
          top: 15,
          right: 16,
          maxWidth: 220,
          textAlign: 'right',
          fontSize: 11,
          fontWeight: 500,
          letterSpacing: 0.2,
          color: 'text.disabled',
          pointerEvents: 'none',
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
        }}
      >
        {stepLabel}
      </Typography>

      {/* Content column: vertically centered and capped to a readable width. The
          inner body keeps a reserved min-height so swapping states never reflows
          the footer. */}
      <Box
        sx={{
          flex: 1,
          minHeight: 0,
          width: '100%',
          maxWidth: 520,
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'center',
          alignItems: 'center',
          // Nudge the centered block down a touch: the top progress bar + step
          // label add visual weight up top, so pure centering reads too high.
          pt: 4,
          overflowY: 'auto',
          overflowX: 'hidden',
        }}
      >
      <Box
        sx={{
          width: '100%',
          minHeight: BODY_MIN_H,
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <AnimatePresence mode="wait">
          <motion.div
            // Waiting and "found" share ONE key: detecting a robot must only
            // update the list in place, not re-fade the whole screen.
            key={viewKey}
            style={{ width: '100%', display: 'flex', justifyContent: 'center' }}
            // While the persistent viz is on screen, DON'T animate the text at
            // all: skipping the enter animation avoids the 1-frame opacity-0
            // flash (flicker) each step would otherwise show. The fade is kept
            // only when entering/leaving a non-viz screen.
            initial={vizVisible ? false : { opacity: 0, y: 4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={vizVisible ? { opacity: 1 } : { opacity: 0, y: -4 }}
            transition={{ duration: vizVisible ? 0 : 0.18 }}
          >
            {effStatus === 'intro' ? (
              <IntroBody />
            ) : effStatus === 'found' ? (
              <SelectReachyBody
                device={effDevice}
                selected={selected}
                onSelect={toggleSelect}
                preparing={effPreparing}
                prepareError={effPrepareError}
                timedOut={false}
              />
            ) : effStatus === 'connect' ? (
              effConnectStep >= CONNECT_N ? (
                <SelectReachyBody
                  device={null}
                  selected={false}
                  onSelect={toggleSelect}
                  preparing={effPreparing}
                  prepareError={effPrepareError}
                  timedOut={waitTimedOut}
                />
              ) : (
                <ConnectStepBody step={CONNECT_STEPS[effConnectStep]} mediaRef={registerMedia} />
              )
            ) : effStatus === 'ready' ? (
              <ReadyBody version={effVersion} imageReady={effImageReady} />
            ) : effStatus === 'flashing' ? (
              <FlashingBody progress={effProgress} />
            ) : effStatus === 'flashed' ? (
              <FlashedBody />
            ) : effStatus === 'done' ? (
              effDoneStep >= DONE_N ? (
                <DoneBody />
              ) : (
                <ConnectStepBody step={DONE_STEPS[effDoneStep]} mediaRef={registerMedia} />
              )
            ) : (
              <ErrorBody raw={effError} />
            )}
          </motion.div>
        </AnimatePresence>
      </Box>
      </Box>

      <ReachyStage
        shotId={shotId}
        state={vizState}
        snapKey={vizSnapKey}
        visible={vizVisible}
        rect={mediaRect}
        card={{
          border: `1px solid ${theme.palette.divider}`,
          background: alpha(theme.palette.text.primary, 0.02),
          radius: 12,
        }}
      />

      <ActionBar back={backAction} primary={primaryAction} />

      <Footer
        downloading={effDownloading}
        version={effVersion ?? osVersion}
        pct={effDownloadPct}
        error={effImageError}
        onRetry={startPrefetch}
      />

      <FlashConfirmDialog
        open={confirmOpen}
        version={effVersion}
        onCancel={() => setConfirmOpen(false)}
        onConfirm={() => {
          setConfirmOpen(false);
          onFlash();
        }}
      />
    </Box>
  );
}

/** Confirmation modal before the destructive write. */
function FlashConfirmDialog({
  open,
  version,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  version: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      maxWidth="xs"
      fullWidth
      slotProps={{
        paper: { sx: { borderRadius: 2, p: 0.5 } },
        backdrop: {
          sx: {
            backdropFilter: 'blur(6px)',
            backgroundColor: (t) => alpha(t.palette.background.default, 0.4),
          },
        },
      }}
    >
      <DialogTitle sx={{ fontWeight: 700, fontSize: '1.5rem' }}>Flash ReachyMiniOS?</DialogTitle>
      <DialogContent>
        <DialogContentText sx={{ color: 'text.secondary', fontSize: '1.0625rem' }}>
          This installs {version ? <B>version {version}</B> : 'the latest version'}. Keep your
          Reachy <S>plugged in</S> until it finishes.
        </DialogContentText>
        <Box
          sx={{
            mt: 1.75,
            display: 'flex',
            alignItems: 'center',
            gap: 1.25,
            px: 1.75,
            py: 1.25,
            borderRadius: 1,
            bgcolor: (t) => alpha(t.palette.primary.main, 0.08),
            border: (t) => `1px solid ${alpha(t.palette.primary.main, 0.35)}`,
          }}
        >
          <WarningAmberRounded sx={{ color: 'primary.main', fontSize: 22, flexShrink: 0 }} />
          <Typography sx={{ fontSize: '0.9375rem', fontWeight: 600, color: 'primary.main' }}>
            This{' '}
            <Box component="span" sx={{ fontWeight: 800 }}>
              erases everything
            </Box>{' '}
            on your Reachy - it can&apos;t be undone.
          </Typography>
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, pb: 2 }}>
        <Button onClick={onCancel} color="inherit" sx={{ borderRadius: 1 }}>
          Cancel
        </Button>
        <Button
          onClick={onConfirm}
          variant="outlined"
          color="primary"
          sx={{ borderRadius: 1 }}
        >
          Flash Reachy
        </Button>
      </DialogActions>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/** Opening screen: explains, in one glance, what the whole flow will do. */
function IntroBody() {
  return (
    <Stack spacing={2} sx={{ ...BODY_STACK_SX, mt: -6 }}>
      <VisualSlot sx={{ mb: 1.5 }}>
        <Box
          component="img"
          src={reachyImg}
          alt="Reachy Mini"
          sx={{ height: '100%', width: 'auto', objectFit: 'contain' }}
        />
      </VisualSlot>
      <Typography sx={TITLE_SX}>Flash your Reachy Mini</Typography>
      <Typography sx={DESC_SX}>
        This installs the latest <S>ReachyMiniOS</S> on your <S>Reachy Mini Wireless</S>{' '}
        <S>over USB</S>. We&apos;ll walk you through connecting it, flashing, and restarting it.
      </Typography>
    </Stack>
  );
}

/**
 * One hardware instruction per screen (the Connect wizard). Purely informational
 * - navigation lives in the shared bottom ActionBar. A fixed-height visual (photo
 * for the error-prone SW1 / USB steps, a large icon otherwise), a big title and a
 * one-line description. The only progress indicator is the unified top bar.
 */
function ConnectStepBody({
  step,
  mediaRef,
}: {
  step: StepItem;
  mediaRef?: (el: HTMLDivElement | null) => void;
}) {
  // Fixed height + top alignment so the media frame sits at the exact same
  // on-screen position across every wizard step - the persistent 3D stage
  // overlays it and must never shift, regardless of the description length.
  return (
    <Stack spacing={2} sx={{ ...BODY_STACK_SX, height: BODY_MIN_H, justifyContent: 'flex-start' }}>
      <VisualSlot>
        {/* Empty frame: the persistent ReachyStage (rendered once at the screen
            root) is positioned right on top of this rectangle. */}
        <MediaFrame frameRef={mediaRef} />
      </VisualSlot>
      {/* Extra top gap: the viz overlay is grown ~15% and overhangs the reserved
          slot, so push the title down to keep clear breathing room below it. */}
      <Typography sx={{ ...TITLE_SX, mt: 3 }}>{step.label}</Typography>
      {step.desc && <Typography sx={DESC_SX}>{step.desc}</Typography>}
      {step.note && (
        <Stack
          direction="row"
          spacing={1}
          sx={{
            alignItems: 'center',
            alignSelf: 'center',
            maxWidth: 360,
            mt: 0.5,
            px: 1.5,
            py: 1,
            textAlign: 'left',
            borderRadius: 1.5,
            border: (t) => `1px solid ${alpha(t.palette.primary.main, 0.35)}`,
            bgcolor: (t) => alpha(t.palette.primary.main, 0.06),
          }}
        >
          <AccessTimeRounded sx={{ fontSize: 18, color: 'primary.main', flexShrink: 0 }} />
          <Typography sx={{ fontSize: '0.8125rem', color: 'text.secondary', textAlign: 'left' }}>
            {step.note}
          </Typography>
        </Stack>
      )}
    </Stack>
  );
}

/** The one shared visual container: fixed size, soft tinted background and a
 * hairline border. Photos fill it edge-to-edge; icons and the Reachy mark sit
 * centered inside. Used by every screen for a consistent composition. */
function MediaFrame({
  children,
  frameRef,
}: {
  children?: ReactNode;
  frameRef?: (el: HTMLDivElement | null) => void;
}) {
  // Transparent spacer: it only reserves the layout footprint so the text
  // never shifts. The visible card + robot are drawn by the persistent
  // ReachyStage overlay (slightly larger, and never remounted / faded).
  return (
    <Box
      ref={frameRef}
      sx={{
        width: MEDIA_W,
        height: MEDIA_H,
        position: 'relative',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
      }}
    >
      {children}
    </Box>
  );
}

/**
 * Live device picker (final Connect screen + detected state, merged). While we
 * poll, the list shows a searching placeholder; once a Reachy enumerates it
 * becomes a selectable row. The bottom Next stays disabled until the user picks
 * it. Recovery (Try again) lives in the bottom ActionBar, not here.
 */
function SelectReachyBody({
  device,
  selected,
  onSelect,
  preparing,
  prepareError,
  timedOut,
}: {
  device: ReachyDevice | null;
  selected: boolean;
  onSelect: () => void;
  preparing: boolean;
  prepareError: string | null;
  timedOut: boolean;
}) {
  const sizeGB = device && device.size > 0 ? Math.max(1, Math.round(device.size / 1e9)) : null;
  const subtitle =
    device?.mode === 'simulated'
      ? 'Simulated device'
      : sizeGB
        ? `Storage ready \u00b7 ${sizeGB} GB`
        : 'Ready to flash';

  return (
    <Stack spacing={2} sx={BODY_STACK_SX}>
      <Typography sx={TITLE_SX}>{device ? 'Reachy found' : 'Looking for your Reachy'}</Typography>
      {/* Reserve a fixed 2-line height (and enough width) so switching between
          the searching and found copy never shifts the layout below. */}
      <Typography sx={{ ...DESC_SX, maxWidth: 400, minHeight: '3em' }}>
        {device ? (
          <>
            Your Reachy is connected and <S>ready to be flashed</S> - select it below to continue.
          </>
        ) : (
          <>
            Make sure it&apos;s <S>powered on</S>, in <B>DOWNLOAD</B> mode, and connected over{' '}
            <B>USB</B>.
          </>
        )}
      </Typography>

      <Box sx={{ width: '100%', maxWidth: 360 }}>
        {prepareError ? (
          <Typography variant="body2" color="error" sx={{ py: 2 }}>
            {humanizeError(prepareError).message}
          </Typography>
        ) : device ? (
          <ButtonBase
            onClick={onSelect}
            aria-pressed={selected}
            sx={{
              width: '100%',
              minHeight: SELECT_ROW_H,
              display: 'flex',
              alignItems: 'center',
              gap: 1.5,
              textAlign: 'left',
              px: 2,
              py: 1,
              borderRadius: '12px',
              border: (t) =>
                `1px solid ${selected ? t.palette.primary.main : t.palette.divider}`,
              bgcolor: (t) =>
                selected ? alpha(t.palette.primary.main, 0.06) : 'background.paper',
              transition: 'border-color .15s ease, background-color .15s ease',
              '&:hover': { borderColor: (t) => t.palette.primary.main },
              '&:focus-visible': {
                outline: (t) => `2px solid ${t.palette.primary.main}`,
                outlineOffset: 2,
              },
            }}
          >
            <Box
              sx={{
                width: 40,
                height: 40,
                flexShrink: 0,
                position: 'relative',
                borderRadius: '50%',
                bgcolor: (t) => alpha(t.palette.text.primary, 0.03),
                border: (t) => `1px solid ${t.palette.divider}`,
                overflow: 'visible',
              }}
            >
              <Box
                component="img"
                src={reachyAvatarImg}
                alt=""
                aria-hidden
                sx={{
                  position: 'absolute',
                  width: '155%',
                  height: 'auto',
                  left: '50%',
                  top: '50%',
                  transform: 'translate(-50%, -60%)',
                  pointerEvents: 'none',
                  userSelect: 'none',
                }}
              />
            </Box>
            <Box sx={{ flex: 1, minWidth: 0 }}>
              <Typography sx={{ fontSize: '1rem', fontWeight: 600, lineHeight: 1.2 }}>
                Reachy Mini
              </Typography>
              <Typography sx={{ fontSize: '0.8125rem', color: 'text.secondary' }}>
                {subtitle}
              </Typography>
            </Box>
            {selected ? (
              <CheckCircleRounded sx={{ fontSize: 22, color: 'primary.main' }} />
            ) : (
              <Box
                sx={{
                  width: 20,
                  height: 20,
                  borderRadius: '50%',
                  border: (t) => `2px solid ${t.palette.divider}`,
                }}
              />
            )}
          </ButtonBase>
        ) : (
          <Stack
            direction="row"
            spacing={1.25}
            sx={{
              minHeight: SELECT_ROW_H,
              alignItems: 'center',
              justifyContent: 'center',
              px: 2,
              py: 1,
              borderRadius: '12px',
              border: (t) => `1px dashed ${t.palette.divider}`,
            }}
          >
            <CircularProgress size={18} thickness={5} sx={{ color: 'text.disabled' }} />
            <Typography sx={{ fontSize: '0.9375rem', color: 'text.secondary' }}>
              {preparing ? 'Reachy detected, preparing storage\u2026' : 'Searching\u2026'}
            </Typography>
          </Stack>
        )}
      </Box>

      {timedOut && !device && !prepareError && (
        <Typography
          sx={{ fontSize: '0.8125rem', color: 'text.secondary', maxWidth: 360, mx: 'auto' }}
        >
          Still nothing? Double-check every step above. The <S>fan</S> is a good tell - if it
          isn&apos;t spinning, the board may be dead.{' '}
          <Typography
            component="button"
            type="button"
            onClick={() => void openUrl(TROUBLESHOOTING_URL)}
            sx={{
              border: 0,
              p: 0,
              background: 'none',
              cursor: 'pointer',
              font: 'inherit',
              color: 'primary.main',
              textDecoration: 'underline',
              textUnderlineOffset: 2,
            }}
          >
            See troubleshooting ↗
          </Typography>
        </Typography>
      )}
    </Stack>
  );
}

function ReadyBody({ version, imageReady }: { version: string | null; imageReady: boolean }) {
  return (
    <Stack spacing={1.5} sx={BODY_STACK_SX}>
      <Typography sx={TITLE_SX}>Install ReachyMiniOS</Typography>
      <Box
        sx={{
          px: 1,
          py: 0.35,
          borderRadius: '6px',
          border: TAG_BORDER,
        }}
      >
        <Typography sx={{ fontSize: '0.9375rem', fontWeight: 600, color: 'text.secondary' }}>
          {version ? `Version ${version}` : imageReady ? 'Latest version' : 'Preparing OS\u2026'}
        </Typography>
      </Box>
    </Stack>
  );
}

function formatElapsed(totalSec: number): string {
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function FlashingBody({ progress }: { progress: FlashProgress | null }) {
  const total = progress?.total ?? 0;
  const written = progress?.written ?? 0;
  const pct = total > 0 ? Math.min(100, Math.round((written / total) * 100)) : null;

  // Elapsed timer: gives a sense of progress even when the write % stalls.
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const start = Date.now();
    const id = setInterval(() => setElapsed(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => clearInterval(id);
  }, []);

  return (
    <Stack spacing={2} sx={{ ...BODY_STACK_SX, mt: 4 }}>
      <Typography sx={TITLE_SX}>Writing image{'\u2026'}</Typography>
      <Box sx={{ width: '100%', maxWidth: 420 }}>
        <FlashProgressBar pct={pct} />
      </Box>
      <Stack
        direction="row"
        spacing={0.75}
        sx={{ alignItems: 'center', justifyContent: 'center', color: 'text.secondary' }}
      >
        <AccessTimeRounded sx={{ fontSize: 15 }} />
        <Typography sx={{ fontSize: '0.8125rem', fontVariantNumeric: 'tabular-nums' }}>
          {formatElapsed(elapsed)} - this usually takes <S>a few minutes</S>
        </Typography>
      </Stack>
      <Typography sx={{ fontSize: '0.8125rem', color: 'text.secondary' }}>
        Keep your Reachy <S>plugged in</S> - don&apos;t power it off
      </Typography>
    </Stack>
  );
}

/** Celebratory screen shown after a successful write. Confirms success and
 * tells the user the next screens will guide the restart; Next starts them. */
function FlashedBody() {
  return (
    <Stack spacing={1.75} sx={BODY_STACK_SX}>
      <Typography sx={TITLE_SX}>Flash complete!</Typography>
      <Typography sx={DESC_SX}>
        <B>ReachyMiniOS</B> was written successfully. In the next steps, we&apos;ll guide you
        through putting your Reachy back to its <S>normal state</S>.
      </Typography>
    </Stack>
  );
}

function DoneBody() {
  return (
    <Stack spacing={1.75} sx={BODY_STACK_SX}>
      <VisualSlot sx={{ mb: 1.5 }}>
        <Box sx={{ position: 'relative', display: 'inline-flex', height: '100%' }}>
          <Box
            component="img"
            src={reachyImg}
            alt="Reachy Mini"
            sx={{ height: '100%', width: 'auto', objectFit: 'contain' }}
          />
          <CheckCircleRounded
            sx={{
              position: 'absolute',
              right: 0,
              bottom: 6,
              fontSize: 40,
              color: 'success.main',
              bgcolor: 'background.default',
              borderRadius: '50%',
            }}
          />
        </Box>
      </VisualSlot>
      <Typography sx={TITLE_SX}>You&apos;re all set!</Typography>
      <Typography sx={DESC_SX}>
        Your Reachy is back to its <S>normal state</S> and running the fresh{' '}
        <B>ReachyMiniOS</B>. You can flash another one whenever you like.
      </Typography>
    </Stack>
  );
}

function ErrorBody({ raw }: { raw: string }) {
  const { title, message } = humanizeError(raw);
  return (
    <Stack spacing={1.25} sx={BODY_STACK_SX}>
      <Typography sx={TITLE_SX}>{title}</Typography>
      <Typography sx={DESC_SX}>{message}</Typography>
      <Typography
        variant="caption"
        sx={{
          display: 'block',
          width: '100%',
          maxWidth: 400,
          p: 1,
          borderRadius: 1,
          bgcolor: 'action.hover',
          color: 'text.secondary',
          textAlign: 'left',
          wordBreak: 'break-word',
        }}
      >
        {raw}
      </Typography>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Action bar (single, fixed navigation zone at the bottom)
// ---------------------------------------------------------------------------

/** The one navigation zone: Back (left) + primary (right), outlined with arrows,
 * positions constant across every screen (installer convention). When only one
 * action exists it is centered. Height is always reserved so screens without
 * actions (e.g. flashing) don't reflow. Every actionable button in the app lives
 * here - nothing floats in the bodies. */
/** A click on Back/Next advances the step. The buttons keep STABLE per-role
 * keys (they never remount between steps), so the click's TouchRipple plays out
 * on the very button you pressed. We only defer the actual navigation by roughly
 * the ripple's visible duration so it isn't cut off by the step swap, and guard
 * against re-entrancy so an impatient double-click can't skip a step (which used
 * to make the steps flicker and the 3D viz snap around). */
const NAV_RIPPLE_MS = 200;

function ActionBar({ back, primary }: { back: BarAction | null; primary: BarAction | null }) {
  const single = Number(!!back) + Number(!!primary) === 1;
  const btnSx = { borderRadius: 1, minWidth: 190, py: 1.1, fontSize: '1rem' } as const;
  // One navigation in flight at a time; the ripple plays during the short delay.
  const pending = useRef(false);
  const defer = (fn: () => void) => () => {
    if (pending.current) return;
    pending.current = true;
    window.setTimeout(() => {
      pending.current = false;
      fn();
    }, NAV_RIPPLE_MS);
  };
  return (
    <Box
      sx={{
        width: '100%',
        maxWidth: 520,
        mx: 'auto',
        height: 64,
        flexShrink: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: single ? 'center' : 'space-between',
        gap: 2,
      }}
    >
      {/* Stable per-role keys: React matches these by key (not sibling position),
          so a single centered button turning into a Back+Next pair never morphs
          the primary into the back one (which used to make the ripple jump onto
          the wrong button). The node persists across steps, so the ripple that a
          click starts animates normally in place. */}
      {back && (
        <Button
          key="back"
          variant="outlined"
          startIcon={<ChevronLeftRounded />}
          onClick={defer(back.onClick)}
          disabled={back.disabled}
          sx={btnSx}
        >
          {back.label}
        </Button>
      )}
      {primary && (
        <Button
          key="primary"
          variant="outlined"
          endIcon={<ChevronRightRounded />}
          onClick={defer(primary.onClick)}
          disabled={primary.disabled}
          sx={btnSx}
        >
          {primary.label}
        </Button>
      )}
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

function Footer({
  downloading,
  version,
  pct,
  error,
  onRetry,
}: {
  downloading: boolean;
  version: string | null;
  pct: number | null;
  error: string | null;
  onRetry: () => void;
}) {
  return (
    <Stack spacing={0.25} sx={{ alignItems: 'center', flexShrink: 0, pt: 0.5, width: '100%' }}>
      <Box sx={{ height: 18, display: 'flex', alignItems: 'center' }}>
        {error ? (
          <Stack direction="row" spacing={0.75} sx={{ alignItems: 'center' }}>
            <Typography variant="caption" color="error">
              Download failed
            </Typography>
            <Button size="small" onClick={onRetry} sx={{ minWidth: 0, py: 0, px: 0.5, fontSize: 11 }}>
              Retry
            </Button>
          </Stack>
        ) : downloading ? (
          <Stack direction="row" spacing={0.75} sx={{ alignItems: 'center' }}>
            <CircularProgress size={11} thickness={5} />
            <Typography variant="caption" color="text.secondary">
              {version ? `Downloading ReachyMiniOS ${version}` : 'Getting the latest OS'}
              {pct !== null ? ` \u00b7 ${pct}%` : '\u2026'}
            </Typography>
          </Stack>
        ) : null}
      </Box>
      <Stack direction="row" spacing={1} sx={{ alignItems: 'center' }}>
        <Typography
          sx={{
            fontSize: 10,
            letterSpacing: 0.3,
            color: 'text.disabled',
            opacity: 0.5,
            userSelect: 'none',
          }}
        >
          For Reachy Mini Wireless
        </Typography>
        <Box sx={{ width: '1px', height: 10, bgcolor: 'divider' }} />
        <Typography
          component="button"
          type="button"
          onClick={() => void openUrl(TROUBLESHOOTING_URL)}
          sx={{
            border: 0,
            p: 0,
            background: 'none',
            cursor: 'pointer',
            fontSize: 10,
            letterSpacing: 0.3,
            color: 'primary.main',
            textDecoration: 'underline',
            textUnderlineOffset: 2,
          }}
        >
          Troubleshooting ↗
        </Typography>
      </Stack>
    </Stack>
  );
}

// ---------------------------------------------------------------------------
// Hero visuals
// ---------------------------------------------------------------------------

/** Fixed-height slot that holds each screen's top visual, keeping the vertical
 * rhythm identical across every screen. */
function VisualSlot({ children, sx }: { children: ReactNode; sx?: SxProps<Theme> }) {
  return (
    <Box
      sx={{
        height: VISUAL_H,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexShrink: 0,
        ...sx,
      }}
    >
      {children}
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

/** The single, unified progress line pinned to the very top of the window.
 * This is the ONLY stepper in the app - every screen (incl. the Connect wizard
 * sub-steps) just moves this one value. */
function StepBar({ value }: { value: number }) {
  return (
    <Box
      sx={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        height: 3,
        zIndex: 1300,
        pointerEvents: 'none',
        // Neutral track so the orange fill reads crisply as progress (matches
        // the mobile app's edge-to-edge bar), instead of MUI's washed-out
        // lightened-primary track.
        bgcolor: (theme) => alpha(theme.palette.text.primary, 0.08),
      }}
    >
      <Box
        sx={{
          height: '100%',
          width: `${value}%`,
          bgcolor: 'primary.main',
          transition: 'width 400ms cubic-bezier(0.4, 0, 0.2, 1)',
        }}
      />
    </Box>
  );
}

/** Rounded progress bar with the % centered inside and a light moving shimmer. */
function FlashProgressBar({ pct }: { pct: number | null }) {
  const theme = useTheme();
  const primary = theme.palette.primary.main;
  const indeterminate = pct === null;
  const value = pct ?? 0;
  // The inset fill covers the centre (where the % sits) past ~54%.
  const overFill = !indeterminate && value >= 54;

  const fillGradient = `linear-gradient(90deg, ${alpha(primary, 0.88)}, ${primary})`;

  return (
    <Box
      sx={{
        position: 'relative',
        width: '100%',
        height: 30,
        borderRadius: 999,
        border: `1px solid ${alpha(primary, 0.4)}`,
        bgcolor: alpha(primary, 0.06),
        boxShadow: `inset 0 1px 2px ${alpha(theme.palette.common.black, 0.04)}`,
      }}
    >
      {/* Inset track so the fill floats inside the outline. */}
      <Box sx={{ position: 'absolute', inset: '3px', borderRadius: 999, overflow: 'hidden' }}>
        {indeterminate ? (
          <motion.div
            style={{
              position: 'absolute',
              top: 0,
              bottom: 0,
              width: '38%',
              borderRadius: 999,
              background: fillGradient,
            }}
            animate={{ left: ['-40%', '102%'] }}
            transition={{ duration: 1.2, repeat: Infinity, ease: 'easeInOut' }}
          />
        ) : (
          <motion.div
            style={{
              position: 'absolute',
              top: 0,
              bottom: 0,
              left: 0,
              borderRadius: 999,
              background: fillGradient,
              overflow: 'hidden',
            }}
            animate={{ width: `${value}%` }}
            transition={{ type: 'spring', stiffness: 120, damping: 20 }}
          />
        )}
      </Box>

      <Box
        sx={{
          position: 'absolute',
          inset: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <Typography
          variant="caption"
          sx={{
            fontWeight: 700,
            fontVariantNumeric: 'tabular-nums',
            letterSpacing: 0.2,
            color: overFill ? theme.palette.primary.contrastText : 'text.primary',
            transition: 'color .2s ease',
          }}
        >
          {indeterminate ? 'Preparing\u2026' : `${value}%`}
        </Typography>
      </Box>
    </Box>
  );
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/** Map a raw backend error to a short, human title + message. Raw stays behind
 * the "Details" toggle. */
function humanizeError(raw: string): { title: string; message: string } {
  const e = raw.toLowerCase();
  if (e.includes('download mode')) {
    return {
      title: 'Reachy not ready',
      message: 'Your Reachy is still in download mode. Re-plug the USB cable and wait a few seconds.',
    };
  }
  if (e.includes('no reachy')) {
    return {
      title: 'No Reachy found',
      message: 'Check the USB cable and that the robot is powered on in download mode.',
    };
  }
  if (/authorization was denied|denied|-128/.test(e)) {
    return {
      title: 'Authorization denied',
      message: 'Admin access is needed to prepare the robot. Try again and approve the prompt.',
    };
  }
  // rpiboot (USB preparation) failures must be checked BEFORE the generic
  // disk-access branch: rpiboot talks to the CM4 over USB, so its errors are
  // not a Full Disk Access / TCC problem and must not be mislabeled as one.
  if (e.includes('rpiboot')) {
    if (e.includes('not found')) {
      return {
        title: 'Preparation tool missing',
        message: "rpiboot isn't installed on this machine (see scripts/fetch-rpiboot.sh).",
      };
    }
    return {
      title: "Couldn't prepare the robot",
      message:
        'Failed to expose the CM4 storage over USB. Unplug and re-plug the USB cable (switch on DOWNLOAD), then try again.',
    };
  }
  if (/operation not permitted|permission|full disk/.test(e)) {
    return {
      title: 'Access blocked',
      message: 'macOS blocked disk access. Approve the prompt and try again.',
    };
  }
  if (/corrupt|deflate|inflate|eocd|invalid zip|central directory|checksum|unexpected eof/.test(e)) {
    return {
      title: 'Image was corrupt',
      message: 'The downloaded OS image was damaged and has been cleared. Relaunch to fetch a fresh copy.',
    };
  }
  if (/invalid argument|os error 22/.test(e)) {
    return {
      title: 'Write rejected',
      message: 'The disk rejected the write. Re-plug the USB cable and try again.',
    };
  }
  return {
    title: 'Flash failed',
    message: 'Something went wrong while flashing. Re-plug the cable and try again.',
  };
}
