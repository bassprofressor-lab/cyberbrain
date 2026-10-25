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
