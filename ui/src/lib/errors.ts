import type { ApiError } from "@/api/client";

/** Which plain-language headline an error gets. The technical text stays available below it. */
export type ErrorKind = "unreachable" | "bad-request" | "not-found" | "write-conflict" | "pii-held" | "ring-cap-exceeded" | "bad-frontmatter" | "policy-refusal" | "profile-mismatch" | "internal";

export function errorKind(e: Pick<ApiError, "status" | "body">): ErrorKind {
  // status 0 is the client's own: the request never got an answer.
  if (e.status === 0 && e.body.code === "internal" && /not reachable/i.test(e.body.message)) return "unreachable";
  switch (e.body.code) {
    case "bad-request":
    case "not-found":
    case "write-conflict":
    case "pii-held":
    case "ring-cap-exceeded":
    case "bad-frontmatter":
    case "policy-refusal":
    case "profile-mismatch":
      return e.body.code;
    default:
      return "internal";
  }
}

/** A note or page that is not there. Trying again cannot change that; going elsewhere can. */
export function isNotFound(e: Pick<ApiError, "status" | "body">): boolean {
  return e.status === 404 || e.body.code === "not-found";
}

/** Whether one of the server's caveats says a check did not run. */
export function hasSkippedCheck(caveats: string[]): boolean {
  return caveats.some((c) => /skipped|disabled|not configured/i.test(c));
}
