/**
 * Who is on the other end: the desktop app on someone's computer, or the headless
 * `kvistcode-server` (no window; `/api/session` says `headless: true`). Wording that points
 * the user to "Kvistcode on your computer" goes through `hw`, which words it for a server.
 * Remembered between visits so the "can't connect" screens know too.
 */

const KEY = "armada.headless";

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false;
  }
}

/** The headless server marks its page (`<meta name="kvistcode-host" content="server">`), so even
 * the pairing screen, before any session exists, knows. */
function servedByServer(): boolean {
  try {
    return document.querySelector('meta[name="kvistcode-host"]')?.getAttribute("content") === "server";
  } catch {
    return false;
  }
}

let headless = servedByServer() || read();

export function isHeadless(): boolean {
  return headless;
}

/** Called with each `/api/session` answer. */
export function setHeadless(value: boolean) {
  headless = value;
  try {
    if (value) localStorage.setItem(KEY, "1");
    else localStorage.removeItem(KEY);
  } catch {
    // Private mode: the wording just falls back to the desktop one next time.
  }
}

/** Unpairing: the next server may be a different kind. */
export function forgetHost() {
  headless = servedByServer();
  try {
    localStorage.removeItem(KEY);
  } catch {
    // ignore
  }
}

const SERVER_WORDING: [RegExp, string][] = [
  [/, or in Kvistcode on your computer\./g, "."],
  [/Your phone is offline/g, "This device is offline"],
  [/Connect to Wi-Fi or mobile data to reach Kvistcode on your computer\./g, "Connect to a network to reach the Kvistcode server."],
  [/Your phone couldn't connect to Kvistcode on your computer\./g, "This browser couldn't connect to the Kvistcode server."],
  [/Kvistcode is open and your computer is awake/g, "The Kvistcode server is running"],
  [/Remote access is on in Settings → Phone & remote/g, "kvistcode-server started without errors (see its log)"],
  [/Your phone is on the same Wi-Fi as your computer, or connected to Tailscale/g, "This device can reach the server's address (same network, or Tailscale)"],
  [/If your computer's network address changed \(new Wi-Fi, router restart\), scan the QR code in Kvistcode again\./g, "If the server's address changed, run kvistcode-server --pair on it and open the new link."],
  [/Turn it on in Settings → Phone & remote; this page reconnects by itself\./g, "Check that kvistcode-server is running; this page reconnects by itself."],
  [/Pair a different computer\?/g, "Pair a different server?"],
  [/Pair a different computer/g, "Pair a different server"],
  [/Only pair with a computer you own\./g, "Only pair with a server you own."],
  [/(?:in|from|on) Kvistcode on your computer/g, "on the server"],
  [/Kvistcode on your computer/g, "Kvistcode on the server"],
  [/revoke access from Settings → Phone & remote on your computer/g, "revoke access by stopping the server and deleting remote.json in its data folder (see docs/HEADLESS.md)"],
  [/Your agents on your computer/g, "Your agents on the server"],
  [/Open it on your computer/g, "Open it on the server"],
  [/on your computer/g, "on the server"],
  [/Sign in on the computer first/g, "Sign in on the server first (see docs/HEADLESS.md)"],
  [/Check that your computer is awake and reachable/g, "Check that the server is running and reachable"],
];

/** `text` as the desktop app words it, or as the headless server needs it. */
export function hw(text: string): string {
  if (!headless) return text;
  return SERVER_WORDING.reduce((s, [from, to]) => s.replace(from, to), text);
}
