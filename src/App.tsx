import { useMemo } from 'react';
import CssBaseline from '@mui/material/CssBaseline';
import useMediaQuery from '@mui/material/useMediaQuery';
import { ThemeProvider } from '@mui/material/styles';

import { darkTheme, lightTheme } from './theme';
import { FlasherScreen } from './screens/FlasherScreen';
import { TitleBar } from './components/TitleBar';

export function App() {
  const prefersDark = useMediaQuery('(prefers-color-scheme: dark)');
  const theme = useMemo(() => (prefersDark ? darkTheme : lightTheme), [prefersDark]);

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <TitleBar />
      <FlasherScreen />
    </ThemeProvider>
  );
}
