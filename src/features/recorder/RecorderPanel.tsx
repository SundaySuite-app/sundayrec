import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { RecordingProgress } from "@/lib/bindings/RecordingProgress";
import type { RecordingEvent } from "@/lib/bindings/RecordingEvent";
import type { RecordingOpts } from "@/lib/bindings/RecordingOpts";

/**
 * Spike B recorder panel — a light UI proving the unified-capture plumbing.
 *
 * Start/stop a recording and watch the live signals the Rust engine emits:
 *   - `recording://started` — ffmpeg's first `size=` line (encoding confirmed),
 *   - `recording://progress` — a rising byte count (rendered as MB + elapsed),
 *   - `recording://silence` — a muted-mixer / weak-signal warning,
 *   - `recording://error` — a classified device/disk/disconnect error.
 *
 * The actual capture is HARDWARE-UNVERIFIED (needs a real mic/camera); this
 * panel is the renderer half and is exercised in tests with mocked events.
 */
export function RecorderPanel() {
  const [running, setRunning] = useState(false);
  const [started, setStarted] = useState(false);
  const [bytes, setBytes] = useState(0);
  const [error, setError] = useState<RecordingEvent | null>(null);
  const [silence, setSilence] = useState<RecordingEvent | null>(null);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const launchError = useRef<string | null>(null);
  const [launchErr, setLaunchErr] = useState<string | null>(null);

  // Subscribe to all four recorder channels for the component's lifetime.
  useEffect(() => {
    const unStarted = listen("recording://started", () => {
      setStarted(true);
      setStartedAt(Date.now());
    });
    const unProgress = listen<RecordingProgress>(
      "recording://progress",
      (event) => setBytes(event.payload.bytes_written),
    );
    const unSilence = listen<RecordingEvent>("recording://silence", (event) =>
      setSilence(event.payload),
    );
    const unError = listen<RecordingEvent>("recording://error", (event) =>
      setError(event.payload),
    );
    return () => {
      void unStarted.then((off) => off());
      void unProgress.then((off) => off());
      void unSilence.then((off) => off());
      void unError.then((off) => off());
    };
  }, []);

  // Tick the elapsed clock once a second while running + started.
  useEffect(() => {
    if (!running || startedAt === null) return;
    const id = setInterval(() => {
      setElapsed(Math.floor((Date.now() - startedAt) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [running, startedAt]);

  const reset = () => {
    setStarted(false);
    setBytes(0);
    setError(null);
    setSilence(null);
    setStartedAt(null);
    setElapsed(0);
    launchError.current = null;
    setLaunchErr(null);
  };

  const start = useCallback(async () => {
    reset();
    // Spike defaults: first/default audio device, audio-only, temp output.
    const opts: RecordingOpts = {
      audio_device_name: "",
      video_device_name: null,
      output_path: "/tmp/sundayrec-spike.m4a",
      stop_on_silence: false,
      silence_threshold_db: null,
      framerate: 30,
      stereo: true,
    };
    try {
      await invoke("start_recording", { opts });
      setRunning(true);
    } catch (e) {
      const msg = String((e as { message?: string })?.message ?? e);
      launchError.current = msg;
      setLaunchErr(msg);
    }
  }, []);

  const stop = useCallback(async () => {
    try {
      await invoke("stop_recording");
    } catch (e) {
      setLaunchErr(String((e as { message?: string })?.message ?? e));
    } finally {
      setRunning(false);
    }
  }, []);

  // Stop the recording if the component unmounts while running.
  useEffect(() => {
    return () => {
      if (running) void invoke("stop_recording").catch(() => {});
    };
  }, [running]);

  const mb = (bytes / (1024 * 1024)).toFixed(1);
  const mmss = `${String(Math.floor(elapsed / 60)).padStart(2, "0")}:${String(
    elapsed % 60,
  ).padStart(2, "0")}`;

  return (
    <section
      className="flex w-full max-w-md flex-col gap-3 rounded-lg border border-zinc-700 p-4"
      aria-label="Opptak"
    >
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">Opptak (Spike B)</h2>
        {running ? (
          <button
            type="button"
            className="rounded bg-red-600 px-3 py-1 text-sm font-medium text-white hover:bg-red-500"
            onClick={() => void stop()}
          >
            Stopp
          </button>
        ) : (
          <button
            type="button"
            className="rounded bg-emerald-600 px-3 py-1 text-sm font-medium text-white hover:bg-emerald-500"
            onClick={() => void start()}
          >
            Start opptak
          </button>
        )}
      </div>

      {launchErr && (
        <p className="text-xs text-red-400" role="alert">
          Kunne ikke starte: {launchErr}
        </p>
      )}

      {running && (
        <div className="flex flex-col gap-1 text-sm">
          <p className="opacity-80">
            {started ? (
              <span className="text-emerald-400">● Tar opp</span>
            ) : (
              <span className="opacity-60">Starter … (venter på ffmpeg)</span>
            )}
          </p>
          <p className="tabular-nums">
            {mb} MB skrevet · {mmss}
          </p>
        </div>
      )}

      {silence && (
        <p
          className="rounded bg-amber-900/40 px-2 py-1 text-xs text-amber-300"
          role="status"
        >
          ⚠ Stillhet: {silence.message}
        </p>
      )}

      {error && (
        <p
          className="rounded bg-red-900/40 px-2 py-1 text-xs text-red-300"
          role="alert"
        >
          Feil ({error.code}): {error.message}
        </p>
      )}

      {!running && !error && (
        <p className="text-xs opacity-50">
          Trykk «Start opptak» — ffmpeg fanger lyd (og evt. kamera) i Rust;
          framdrift, stillhet og feil strømmes som hendelser.
        </p>
      )}
    </section>
  );
}
