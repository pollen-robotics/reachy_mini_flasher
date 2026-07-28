import { useCallback, useEffect, useRef, useState } from 'react';
import Box from '@mui/material/Box';
import Button from '@mui/material/Button';
import LinearProgress from '@mui/material/LinearProgress';
import Paper from '@mui/material/Paper';
import Stack from '@mui/material/Stack';
import Typography from '@mui/material/Typography';
import { alpha, useTheme } from '@mui/material/styles';

import {
  getAppUpdateInfo,
  installAppUpdate,
  onAppUpdateAvailable,
  onAppUpdateError,
  onAppUpdateProgress,
  type AppUpdateInfo,
} from '@/lib/api';
import updateBoxImg from '@/assets/reachy-update-box.png';

type Phase = 'available' | 'installing' | 'restarting' | 'error';

/**
 * Forced-update overlay, mirroring `reachy_mini_tray`'s `update.html` but in
 * MUI. Rendered above the whole app: when the backend's startup check finds a
 * newer signed bundle it emits `app-update:available` and we surface a
 * blocking card. The single "Install and restart" action downloads + verifies
 * + installs the update (streaming progress) then the app relaunches itself.
 */
export function UpdateOverlay() {
  const theme = useTheme();
  const accent = theme.palette.primary.main;

  const [info, setInfo] = useState<AppUpdateInfo | null>(null);
  const [phase, setPhase] = useState<Phase>('available');
  const [status, setStatus] = useState('');
  const [percent, setPercent] = useState<number | null>(null);
  const [showBar, setShowBar] = useState(false);
  const installingRef = useRef(false);

  useEffect(() => {
    let mounted = true;
    const unlisteners: Array<() => void> = [];

    // The overlay may mount after the `available` event fired, so also pull
    // the cached info via the getter.
    getAppUpdateInfo()
      .then((i) => {
        if (mounted && i) setInfo(i);
      })
      .catch(() => {});

    onAppUpdateAvailable((i) => {
      if (mounted) setInfo(i);
    }).then((u) => unlisteners.push(u));

    onAppUpdateProgress((p) => {
      if (!mounted) return;
      setShowBar(true);
      if (typeof p.percent === 'number') {
        setPercent(p.percent);
        setStatus(`Downloading\u2026 ${p.percent}%`);
      } else {
        setPercent(null);
        setStatus('Downloading\u2026');
      }
    }).then((u) => unlisteners.push(u));

    onAppUpdateError((message) => {
      if (!mounted) return;
      installingRef.current = false;
      setPhase('error');
      setShowBar(false);
      setPercent(null);
      setStatus(message);
    }).then((u) => unlisteners.push(u));

    return () => {
      mounted = false;
      unlisteners.forEach((u) => u());
    };
  }, []);

  const install = useCallback(async () => {
    if (installingRef.current) return;
    installingRef.current = true;
    setPhase('installing');
    setShowBar(true);
    setPercent(null);
    setStatus('Preparing download\u2026');
    try {
      await installAppUpdate();
      // On success the process re-execs; this mostly won't be reached, but if
      // it is (slow relaunch) show a terminal "restarting" state.
      setPhase('restarting');
      setStatus('Restarting\u2026');
    } catch (e) {
      installingRef.current = false;
      setPhase('error');
      setShowBar(false);
      setPercent(null);
      setStatus(e instanceof Error ? e.message : String(e));
    }
  }, []);

  if (!info) return null;

  const installing = phase === 'installing' || phase === 'restarting';
  const buttonLabel =
    phase === 'error' ? 'Retry install' : phase === 'available' ? 'Install and restart' : 'Installing\u2026';

  return (
    <Box
      sx={{
        position: 'fixed',
        inset: 0,
        zIndex: (t) => t.zIndex.modal + 10,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        p: 3,
        bgcolor: alpha(theme.palette.background.default, 0.6),
        backdropFilter: 'blur(10px)',
        WebkitBackdropFilter: 'blur(10px)',
      }}
    >
      <Paper
        elevation={0}
        sx={{
          width: '100%',
          maxWidth: 420,
          px: 4,
          py: 4,
          borderRadius: 3,
          border: `1px solid ${theme.palette.divider}`,
          textAlign: 'center',
          boxShadow: `0 24px 60px ${alpha(theme.palette.common.black, theme.palette.mode === 'dark' ? 0.5 : 0.18)}`,
        }}
      >
        <Stack spacing={0} sx={{ alignItems: 'center' }}>
          <Box
            component="img"
            src={updateBoxImg}
            alt="Reachy Mini in a box"
            sx={{ width: 120, height: 120, objectFit: 'contain', mb: 1.5, pointerEvents: 'none' }}
          />

          <Typography variant="h4" sx={{ mb: 0.5 }}>
            Update available
          </Typography>
          <Typography variant="body2" color="text.secondary" sx={{ maxWidth: 300, mb: 2 }}>
            A new version of Reachy Mini Flasher is ready to install.
          </Typography>

          <Stack direction="row" spacing={1} sx={{ alignItems: 'center', mb: info.notes ? 1.5 : 2.5 }}>
            <VersionPill label={`v${info.current}`} />
            <Typography aria-hidden component="span" sx={{ color: 'text.disabled' }}>
              {'\u2192'}
            </Typography>
            <VersionPill label={`v${info.version}`} highlight />
          </Stack>

          {info.notes.trim().length > 0 && (
            <Box
              sx={{
                maxWidth: 320,
                maxHeight: 72,
                overflowY: 'auto',
                mb: 2,
                color: 'text.secondary',
                fontSize: '0.8125rem',
                lineHeight: 1.5,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
              }}
            >
              {info.notes.trim()}
            </Box>
          )}

          {showBar && (
            <Box sx={{ width: '100%', maxWidth: 300, mb: 1 }}>
              <LinearProgress
                variant={percent === null ? 'indeterminate' : 'determinate'}
                value={percent ?? 0}
                sx={{
                  height: 6,
                  borderRadius: 999,
                  bgcolor: alpha(accent, 0.14),
                  '& .MuiLinearProgress-bar': { borderRadius: 999 },
                }}
              />
            </Box>
          )}

          <Typography
            variant="caption"
            sx={{
              minHeight: 16,
              mb: 2,
              color: phase === 'error' ? 'error.main' : 'text.disabled',
            }}
          >
            {status}
          </Typography>

          <Button
            variant="outlined"
            color="primary"
            onClick={install}
            disabled={installing}
            sx={{ minWidth: 220, borderWidth: 1.5, '&:hover': { borderWidth: 1.5 } }}
          >
            {buttonLabel}
          </Button>
        </Stack>
      </Paper>
    </Box>
  );
}

function VersionPill({ label, highlight }: { label: string; highlight?: boolean }) {
  const theme = useTheme();
  const accent = theme.palette.primary.main;
  return (
    <Box
      component="span"
      sx={{
        fontSize: '0.75rem',
        fontWeight: 600,
        fontVariantNumeric: 'tabular-nums',
        px: 1.25,
        py: 0.4,
        borderRadius: 999,
        color: highlight ? accent : 'text.secondary',
        bgcolor: highlight
          ? alpha(accent, 0.14)
          : alpha(theme.palette.text.primary, theme.palette.mode === 'dark' ? 0.12 : 0.08),
      }}
    >
      {label}
    </Box>
  );
}
