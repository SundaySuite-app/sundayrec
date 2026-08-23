/**
 * Audio input helpers the shell shares with the backend's enumeration.
 *
 * ## The blink-open that had to go
 *
 * `getAudioDevices()` used to open `getUserMedia({ audio: true })` for a blink
 * and throw the stream away immediately — not to capture anything, but because
 * `enumerateDevices()` returns EMPTY LABELS until a page has been granted a
 * microphone. So every device picker, every home-page status check and every
 * recording-overlay device name silently made the webview an owner of the
 * default input device for a moment.
 *
 * On macOS that moment is not bounded: WebKit tears a CoreAudio input down
 * asynchronously, so the "blink" could still be holding the device when the
 * recorder opened it — the 2026-07-31 Qu-5 incident, where a 32-channel mixer
 * stayed pinned to the 2-channel format gUM had negotiated.
 *
 * The list comes from the backend (`list_audio_devices` -> cpal, plus ASIO on
 * Windows), which is the same enumeration the recorder itself resolves against.
 * No permission prompt, no stream, real labels, and - for free - the real
 * channel count per device. The renderer holds no microphone at all.
 *
 * ## What fase B removed from this file, and what went with it
 *
 * `getAudioDevices()` itself is gone: `app/state/devices.ts` reads
 * `list_audio_devices` and shapes it for the screens, and two shapers over one
 * list is the seam class this whole redesign exists to stop making.
 *
 * ⚠️ `healStoredDeviceId()` is gone too, and that one was BEHAVIOUR, not a
 * duplicate. It re-pointed a stored `deviceId` that no longer resolved by
 * matching on the stored NAME - which mattered twice: Windows reassigns device
 * ids after a reboot or a driver update, and ids written by a pre-backend build
 * are Web Audio hashes. The channel-grid L/R picks are keyed BY id, so without
 * the heal a Qu-5 rig silently reverts to channels 1/2 and nobody finds out
 * until the recording is of the wrong source. The new shell never called it
 * (and could not have: it read a module-scope `settings` mirror the shell does
 * not populate). Rebuilding it over `app/state/settings.ts` is a named restanse
 * - see docs/APP-SHELL.md, "Etter byttet".
 */

/**
 * Is this the machine's own microphone rather than a mixer / interface?
 *
 * Drives the "Ikke anbefalt" badge in the picker and the onboarding wizard's
 * default pick. The old test was `/built-in|innebygd|default/i` against Web
 * Audio labels, which said «Built-in Microphone» — CoreAudio calls the same
 * device «MacBook Pro Microphone», and Windows «Microphone Array», so the badge
 * would have quietly stopped appearing on exactly the machines that need it.
 */
export function isBuiltInDevice(label: string): boolean {
  return /built-?in|innebygd|internal|default|standard|macbook|imac|mac ?(mini|studio|pro)|microphone array|mikrofonrekke/i.test(label)
}
