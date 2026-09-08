/**
 * The terminal token, taken from the address once and kept for this tab.
 *
 * `cyberbrain serve --terminal` prints an address with the token in its *fragment*, which a
 * browser keeps to itself and never sends to a server. The page takes it on arrival and
 * removes it from the address, so it is not left in a bookmark, in the history, or on screen
 * behind somebody.
 *
 * Held in `sessionStorage` rather than a variable, and the reason is a defect that showed up
 * the first time this was tried in a browser: reloading the page threw the token away with
 * it, and the terminal was gone until somebody reopened the whole project from the tray. A
 * first draft avoided storage on the grounds that anything else on this origin could read
 * it — but the only thing on this origin is this page, served out of the binary, and a
 * `sessionStorage` entry dies with the tab and never reaches disk. That trade is the right
 * way round.
 *
 * Never `localStorage`: a token that outlives the run of `serve` that minted it is a token
 * that is wrong by the next run, and one that outlives the browser is one on disk.
 */
const KEY = "cyberbrain.terminal-token";

/** Take it from a route's query if it is there. Returns true when the address should be
 * rewritten without it. */
export function captureToken(query: URLSearchParams): boolean {
    const found = query.get("t");
    if (!found) return false;
    try {
        sessionStorage.setItem(KEY, found);
    } catch {
        // Private mode, or storage refused. The token still works for this page load; only
        // a reload will lose it, which is better than not working at all.
    }
    remembered = found;
    return true;
}

let remembered: string | null = null;

export function terminalToken(): string | null {
    if (remembered) return remembered;
    try {
        remembered = sessionStorage.getItem(KEY);
    } catch {
        remembered = null;
    }
    return remembered;
}
