/**
 * Which OS the app is running on.
 *
 * Needed because several screens describe the **system prompts the user is about
 * to see**, and those are not the same thing on each platform: macOS asks for an
 * administrator password, Windows raises User Account Control and, the first
 * time, installs a USB driver. Naming the wrong OS in that copy is worse than
 * saying nothing - it tells the user to look for a dialog that will never come.
 *
 * Read from the webview's user agent rather than a backend command so it
 * resolves **synchronously**: this drives text rendered on first paint, and an
 * async answer would show the wrong OS for a frame before correcting itself.
 * WebView2 always reports `Windows NT`, WKWebView always reports `Macintosh`.
 */
export type HostOs = 'macos' | 'windows' | 'other';

export const HOST_OS: HostOs = (() => {
  const ua = typeof navigator === 'undefined' ? '' : navigator.userAgent;
  if (/Windows/i.test(ua)) return 'windows';
  if (/Macintosh|Mac OS X/i.test(ua)) return 'macos';
  return 'other';
})();

export const IS_WINDOWS = HOST_OS === 'windows';

/**
 * The OS name to put in front of the user.
 *
 * Falls back to "Your system" rather than guessing: the app only ships for
 * macOS and Windows, so anything else is a dev build on Linux, and vague copy
 * beats confidently wrong copy.
 */
export const OS_NAME = HOST_OS === 'windows' ? 'Windows' : HOST_OS === 'macos' ? 'macOS' : 'Your system';
