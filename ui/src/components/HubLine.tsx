/**
 * Where this machine's audit trail goes, on the screen the person already has open.
 *
 * Somebody installing a hub asked how to tell, from the dashboard, whether this machine was
 * a hub, a client, or licensed. From this screen you could not tell any of it, and the
 * answer was a command — which is no answer at all for the person the collection is *about*.
 *
 * It sits in the sidebar rather than on the Status screen because "does anything leave this
 * machine" is not a thing to go and look up. It is a thing to be able to see.
 */
import { useEffect, useState } from "react";
import { api } from "@/api/client";
import type { HubStatus } from "@/api/types";
import { relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";

export function HubLine() {
  const t = useT();
  const [hub, setHub] = useState<HubStatus | null>(null);

  useEffect(() => {
    let alive = true;
    // Quietly: a store that is not enrolled is the ordinary case, and a failure here must
    // never be a banner on somebody's notes.
    api
      .hubStatus()
      .then((h) => alive && setHub(h))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  if (!hub) return null;

  if (!hub.enrolled) {
    return (
      <div title={t.app.hubAloneWhy} className="leading-snug">
        {t.app.hubAlone}
      </div>
    );
  }

  // The host is enough to recognise it by; the whole URL would wrap over three lines in a
  // sidebar this wide and tell nobody anything more.
  let host = hub.hub ?? "";
  let link: string | null = null;
  try {
    const u = new URL(host);
    host = u.host;
    // Only the two schemes a hub is ever reached over. The address comes from this store's
    // own config file, which is text on disk that anything could have written, and a link
    // is the one place where "javascript:" would mean something.
    if (u.protocol === "http:" || u.protocol === "https:") link = u.href;
  } catch {
    /* not a URL we can shorten; show it as it was configured */
  }

  return (
    <div title={`${hub.hub ?? ""}${hub.device ? ` · ${hub.device}` : ""}\n\n${t.app.hubWhatLeaves}`} className="leading-snug">
      <div>
        {t.app.hubReports}{" "}
        {link ? (
          <a href={link} className="text-fg-muted font-medium underline decoration-dotted underline-offset-2">
            {host}
          </a>
        ) : (
          <span className="text-fg-muted font-medium">{host}</span>
        )}
      </div>
      <div>{hub.last_delivery ? t.app.hubLastDelivery(relTime(hub.last_delivery)) : t.app.hubNothingYet}</div>
    </div>
  );
}
