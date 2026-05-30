import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import type { FfmpegHealth as FfmpegHealthInfo } from "@/lib/bindings/FfmpegHealth";

/**
 * Spike A diagnostics: probe the bundled ffmpeg sidecar via `ffmpeg_health` and
 * show whether it resolved + its version banner. Proves the externalBin wiring
 * end-to-end before the recorder (Spike B) relies on it.
 */
export function FfmpegHealth() {
  const { data, isLoading, isError } = useQuery<FfmpegHealthInfo>({
    queryKey: ["ffmpeg_health"],
    queryFn: () => invoke<FfmpegHealthInfo>("ffmpeg_health"),
  });

  if (isLoading) {
    return <p className="text-sm opacity-70">ffmpeg: sjekker …</p>;
  }

  if (isError || !data) {
    return <p className="text-sm text-red-400">ffmpeg: helsesjekk feilet ✗</p>;
  }

  if (data.available) {
    return (
      <p className="text-sm text-emerald-400" title={data.path}>
        ffmpeg: {data.version ?? "ukjent versjon"} ✓
      </p>
    );
  }

  return (
    <p className="text-sm text-red-400" title={data.path}>
      ffmpeg: ikke funnet ✗
    </p>
  );
}
