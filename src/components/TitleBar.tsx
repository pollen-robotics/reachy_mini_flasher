import Box from '@mui/material/Box';

/**
 * Empty, transparent draggable strip pinned to the top of the window.
 * With `titleBarStyle: Overlay` the content extends under the native macOS
 * traffic-light buttons, so this strip only exists to let the user move the
 * frameless window. Dragging is handled natively by Tauri via the
 * `data-tauri-drag-region` attribute (requires `core:window:allow-start-dragging`).
 */
export function TitleBar() {
  return (
    <Box
      data-tauri-drag-region
      sx={{
        position: 'fixed',
        top: 0,
        left: 0,
        right: 0,
        height: 40,
        userSelect: 'none',
        WebkitAppRegion: 'drag',
        zIndex: 1200,
      }}
    />
  );
}
