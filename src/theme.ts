import { createTheme, type Theme } from '@mui/material/styles';

import { FONT_WEIGHT, TYPO } from '@/design/tokens';

/**
 * Minimal but production-shaped MUI theme.
 *
 * Two instances (light + dark) built from the same accent, selected at
 * runtime via `prefers-color-scheme`. Keeping both pre-built avoids a
 * visible flash when the user toggles their system appearance.
 *
 * Typography foundation
 * ─────────────────────
 * The MUI variant scale (`h1…h6`, `subtitle*`, `body*`, `caption`,
 * `overline`, `button`) is mapped onto the shared `TYPO` token scale so
 * that a bare `<Typography variant="h3">` is already on-spec WITHOUT a
 * per-call `sx={{ fontSize, fontWeight }}`. This is the "standard by
 * default" layer; existing call-sites that still hand-roll `fontSize`
 * keep working (their `sx` wins) and get migrated to variants
 * incrementally. The token scale stays the single source of truth -
 * the theme just exposes it through MUI's variant system.
 *
 * Kept in sync with `reachy_mini_mobile_app/src/theme.ts` so the flasher
 * shares the exact same visual identity; the only difference is the tokens
 * import path (this project keeps them at `@/design/tokens`).
 */

const ACCENT = '#FF9500'; // Pollen-ish orange, matches the desktop app.
const RADIUS = 12;
const FONT_FAMILY =
  '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", sans-serif';

function buildTheme(mode: 'light' | 'dark'): Theme {
  const isDark = mode === 'dark';
  return createTheme({
    palette: {
      mode,
      primary: { main: ACCENT },
      background: {
        // Lighter "canvas" tone so the contrast with the white
        // cards (`paper`) is subtle - the cards still pop but
        // the body doesn't feel "hard" grey. The dark mode
        // counterpart bumps from pitch black to a softer near-
        // black so the cards (#1a1a1a) still stand out without
        // the body crushing into the OLED's true black.
        default: isDark ? '#101013' : '#fafafa',
        paper: isDark ? '#1a1a1a' : '#ffffff',
      },
      text: {
        primary: isDark ? '#f5f5f5' : '#111111',
        secondary: isDark ? 'rgba(255,255,255,0.72)' : 'rgba(0,0,0,0.65)',
      },
      divider: isDark ? 'rgba(255,255,255,0.08)' : 'rgba(0,0,0,0.08)',
    },
    typography: {
      fontFamily: FONT_FAMILY,
      // Headings: the title scale. `h3` (TYPO.xxl bold) is the canonical
      // "screen title" used by `IllustratedState` and most section heroes.
      h1: { fontSize: TYPO.display, fontWeight: FONT_WEIGHT.bold, lineHeight: 1.15, letterSpacing: '-0.4px' },
      h2: { fontSize: TYPO.hero, fontWeight: FONT_WEIGHT.bold, lineHeight: 1.18, letterSpacing: '-0.3px' },
      h3: { fontSize: TYPO.xxl, fontWeight: FONT_WEIGHT.bold, lineHeight: 1.2, letterSpacing: '-0.2px' },
      h4: { fontSize: TYPO.xl, fontWeight: FONT_WEIGHT.semibold, lineHeight: 1.25, letterSpacing: '-0.2px' },
      h5: { fontSize: TYPO.lg, fontWeight: FONT_WEIGHT.semibold, lineHeight: 1.3 },
      h6: { fontSize: TYPO.md, fontWeight: FONT_WEIGHT.semibold, lineHeight: 1.35 },
      // Supporting copy.
      subtitle1: { fontSize: TYPO.md, fontWeight: FONT_WEIGHT.medium, lineHeight: 1.4 },
      subtitle2: { fontSize: TYPO.sm, fontWeight: FONT_WEIGHT.semibold, lineHeight: 1.4 },
      body1: { fontSize: TYPO.md, fontWeight: FONT_WEIGHT.regular, lineHeight: 1.5 },
      body2: { fontSize: TYPO.body, fontWeight: FONT_WEIGHT.regular, lineHeight: 1.5 },
      caption: { fontSize: TYPO.xs, lineHeight: 1.4 },
      overline: {
        fontSize: TYPO.tiny,
        fontWeight: FONT_WEIGHT.semibold,
        lineHeight: 1.2,
        letterSpacing: '0.5px',
        textTransform: 'uppercase',
      },
      button: { textTransform: 'none', fontWeight: FONT_WEIGHT.semibold, fontSize: TYPO.md },
    },
    shape: { borderRadius: RADIUS },
    components: {
      MuiCssBaseline: {
        styleOverrides: {
          /**
           * Force `html` AND `body` to carry the canvas colour. Without
           * this, MUI's default CssBaseline only paints `body`, which
           * leaves the WKWebView / wry surface area BEHIND `html`
           * exposed in the iOS notch + home indicator zones (and in
           * Android's system bar zones once edge-to-edge kicks in on
           * API 35+). The visible bug is two white bands top + bottom
           * of the screen in dark mode while the rest of the app is
           * almost black. Painting `html` fixes that everywhere.
           *
           * `min-height: 100dvh` (with `100vh` fallback for older
           * WebKits) keeps the canvas filled across the dynamic
           * viewport: when the iOS keyboard slides up the layout
           * viewport shrinks, `dvh` follows, and the bg keeps
           * filling. Plain `100vh` would freeze at the initial
           * height and leak the system bg under the keyboard.
           *
           * Margin reset is defensive: MUI already zeroes the body
           * margin via the standard CssBaseline, but spelling it out
           * makes this block self-contained for anyone reading it
           * outside the MUI defaults context.
           */
          // Emotion accepts an array of values for the same property
          // and emits both CSS declarations in order, so older WebKits
          // (which don't know `dvh`) silently fall back to `vh`. With
          // the object syntax a second key would just override the
          // first, defeating the whole fallback intent.
          html: {
            backgroundColor: isDark ? '#101013' : '#fafafa',
            minHeight: ['100vh', '100dvh'],
          },
          body: {
            backgroundColor: isDark ? '#101013' : '#fafafa',
            minHeight: ['100vh', '100dvh'],
            margin: 0,
          },
          '#root': {
            minHeight: ['100vh', '100dvh'],
          },
        },
      },
      MuiButton: {
        defaultProps: { disableElevation: true },
        styleOverrides: {
          root: { borderRadius: RADIUS, paddingInline: 20, paddingBlock: 10 },
        },
      },
      MuiPaper: {
        styleOverrides: {
          root: { backgroundImage: 'none' },
        },
      },
      MuiCard: {
        styleOverrides: {
          root: { borderRadius: RADIUS },
        },
      },
    },
  });
}

export const lightTheme = buildTheme('light');
export const darkTheme = buildTheme('dark');
