/**
 * Dictation: a microphone button beside a text field.
 *
 * The browser records (MediaRecorder, WebM/Opus where it can), the recording goes to this
 * page's own server (`POST /transcribe`), which hands it to the local whisper-server named
 * in `[transcription]`. Nothing is written by dictating; the text is appended to the field
 * and saving stays the ordinary write with its PII check.
 *
 * The button is not drawn at all while transcription is off, so a store without a
 * whisper-server shows no control that can only fail.
 */
import { useEffect, useRef, useState } from "react";
import { api } from "@/api/client";
import type { TranscriptionStatus } from "@/api/client";
import { useT } from "@/lib/i18n";
import { Icon } from "@/components/icons";
export { appendDictation } from "@/lib/dictation";

let statusOnce: Promise<TranscriptionStatus | null> | null = null;
function transcriptionStatus(): Promise<TranscriptionStatus | null> {
  statusOnce ??= api.transcription().catch(() => null);
  return statusOnce;
}

type Phase = "idle" | "recording" | "working";

export function DictateButton({ onText, className }: { onText: (text: string) => void; className?: string }) {
  const t = useT();
  const [enabled, setEnabled] = useState(false);
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const rec = useRef<MediaRecorder | null>(null);
  const chunks = useRef<Blob[]>([]);

  useEffect(() => {
    let live = true;
    transcriptionStatus().then((s) => live && setEnabled(!!s?.enabled));
    return () => {
      live = false;
      rec.current?.stream.getTracks().forEach((tr) => tr.stop());
    };
  }, []);

  if (!enabled) return null;

  async function start() {
    setError(null);
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch {
      setError(t.dictate.noMic);
      return;
    }
    const type = ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"].find((m) => MediaRecorder.isTypeSupported(m));
    const r = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
    chunks.current = [];
    r.ondataavailable = (e) => e.data.size && chunks.current.push(e.data);
    r.onstop = async () => {
      stream.getTracks().forEach((tr) => tr.stop());
      const blob = new Blob(chunks.current, { type: r.mimeType || "audio/webm" });
      setPhase("working");
      try {
        const d = await api.dictate(blob);
        if (d.text.trim()) onText(d.text);
        else setError(t.dictate.nothingHeard);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setPhase("idle");
      }
    };
    rec.current = r;
    r.start();
    setPhase("recording");
  }

  function stop() {
    rec.current?.stop();
    rec.current = null;
  }

  const label = phase === "recording" ? t.dictate.stop : phase === "working" ? t.dictate.working : t.dictate.start;
  return (
    <span className={`inline-flex items-center gap-2 ${className ?? ""}`}>
      <button
        type="button"
        className={`btn btn-ghost btn-sm ${phase === "recording" ? "text-danger" : ""}`}
        onClick={phase === "recording" ? stop : start}
        disabled={phase === "working"}
        aria-pressed={phase === "recording"}
        title={label}
        aria-label={label}
      >
        <Icon name="mic" size={16} className={phase === "recording" ? "animate-pulse" : undefined} />
        <span className="hidden sm:inline">{label}</span>
      </button>
      {error ? (
        <span className="text-2xs text-danger" role="alert">
          {error}
        </span>
      ) : null}
    </span>
  );
}
