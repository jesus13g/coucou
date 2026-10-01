// The few words that differ between the Windows and Linux builds. The webview's
// user agent says which one we are in: WebView2 reports Windows, WebKitGTK
// reports Linux (X11). Everything else in the front end is shared.

export const IS_LINUX = typeof navigator !== "undefined" && /Linux|X11/.test(navigator.userAgent);

/** Where API keys are kept, as a phrase that fits after "in the". */
export const KEY_STORE = IS_LINUX
  ? "system keyring (Secret Service)"
  : "Windows Credential Manager";

/** File name of the Claude Code relay. */
export const HOOK_EXE = IS_LINUX ? "coucou-hook" : "coucou-hook.exe";
