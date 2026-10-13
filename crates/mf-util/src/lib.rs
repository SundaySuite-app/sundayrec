//! Media Foundation-hjelpere for kamera-cratene: oppstart, MMCSS og bufferlås.
//!
//! **Opphav:** egen kode, skrevet for SundayRec fra Microsofts dokumentasjon
//! (lenkene står ved hver del). Den erstatter Caps `cap-mediafoundation-utils`,
//! som er AGPL og som **ikke er lest** — se `crates/camera-windows/UPSTREAM.md`.
//! Lisens: MIT, som resten av SundayRec.
//!
//! Det de importerte cratene (`cap-camera-mediafoundation`,
//! `cap-camera-windows`) trenger herfra, er bufferlåsen: `buffer.lock()` på en
//! `IMFMediaBuffer`, som gir en vakt som derefererer til de gyldige bytene og
//! låser opp når den slippes. Oppstartsvakten og MMCSS-vakten er for laget som
//! kommer over dem (C1c/C3a): opptakstråden skal registreres hos MMCSS, og
//! Media Foundation skal startes og stoppes i par.
//!
//! Alt her er Windows-only; på andre plattformer er craten tom.

#![cfg(windows)]

use std::{marker::PhantomData, ops::Deref, ptr, slice};

use windows::Win32::{
    Foundation::HANDLE,
    Media::MediaFoundation::{
        IMFMediaBuffer, MF_VERSION, MFSTARTUP_FULL, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
    },
    System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
};
use windows_core::{HSTRING, Result};

// ── Bufferlås ────────────────────────────────────────────────────────────────
// https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfmediabuffer-lock
// https://learn.microsoft.com/en-us/windows/win32/api/mfobjects/nf-mfobjects-imfmediabuffer-unlock
//
// Fra dokumentasjonen: pekeren er gyldig så lenge låsen holdes; `Unlock` skal
// kalles én gang per `Lock`; de gyldige dataene er `pcbCurrentLength` byte
// (som kan være mindre enn `pcbMaxLength`); bufferen eier minnet. Låsen
// synkroniserer IKKE tråder.

/// `lock()` på en `IMFMediaBuffer`.
pub trait IMFMediaBufferExt {
    /// Låser bufferen og gir de gyldige bytene (`pcbCurrentLength`). Låses opp
    /// når vakten slippes.
    fn lock(&self) -> Result<IMFMediaBufferLock<'_>>;
}

impl IMFMediaBufferExt for IMFMediaBuffer {
    fn lock(&self) -> Result<IMFMediaBufferLock<'_>> {
        let mut data: *mut u8 = ptr::null_mut();
        let mut current_len: u32 = 0;
        // SAFETY: ut-pekerne peker på lokale variabler som lever gjennom kallet;
        // maks-lengden trengs ikke og sendes som NULL, slik dokumentasjonen
        // tillater.
        unsafe { self.Lock(&mut data, None, Some(&mut current_len)) }?;
        Ok(IMFMediaBufferLock {
            buffer: self,
            data,
            len: if data.is_null() {
                0
            } else {
                current_len as usize
            },
            _not_send: PhantomData,
        })
    }
}

/// En låst `IMFMediaBuffer`. Derefererer til de gyldige bytene; kaller
/// `Unlock` i `Drop`, nøyaktig én gang.
pub struct IMFMediaBufferLock<'a> {
    buffer: &'a IMFMediaBuffer,
    data: *mut u8,
    len: usize,
    // Vakten skal låses opp av den som låste; ikke send den til en annen tråd.
    _not_send: PhantomData<*const ()>,
}

impl Deref for IMFMediaBufferLock<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        if self.data.is_null() {
            return &[];
        }
        // SAFETY: `Lock` lyktes, så `data` peker på minst `len` gyldige byte
        // til `Unlock` kalles — og det skjer først i `drop`, etter at lånet
        // herfra er borte.
        unsafe { slice::from_raw_parts(self.data, self.len) }
    }
}

impl Drop for IMFMediaBufferLock<'_> {
    fn drop(&mut self) {
        // SAFETY: parer `Lock` fra `IMFMediaBufferExt::lock`. En feil her kan
        // ikke meldes fra `drop`, og bufferen er like gyldig etterpå.
        let _ = unsafe { self.buffer.Unlock() };
    }
}

impl std::fmt::Debug for IMFMediaBufferLock<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IMFMediaBufferLock")
            .field("len", &self.len)
            .finish()
    }
}

// ── Oppstart ─────────────────────────────────────────────────────────────────
// https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfstartup
// https://learn.microsoft.com/en-us/windows/win32/api/mfapi/nf-mfapi-mfshutdown
//
// Fra dokumentasjonen: `MFStartup(MF_VERSION, …)` før Media Foundation brukes;
// `MFShutdown` én gang per vellykket `MFStartup`; ikke fra statiske
// konstruktører og ikke fra arbeidskø-tråder. `E_NOTIMPL` betyr at
// mediekomponentene mangler (N/KN-utgaver, Windows Server uten funksjonen).
// MFStartup initialiserer ikke COM; det gjør den som kaller.

/// Holder Media Foundation startet. `MFShutdown` kalles når vakten slippes.
#[derive(Debug)]
#[must_use = "Media Foundation is shut down again when the guard is dropped"]
pub struct MediaFoundation(());

impl MediaFoundation {
    /// `MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)` — kamerafangst trenger ikke
    /// sokkelbiblioteket.
    pub fn startup() -> Result<Self> {
        Self::startup_with(MFSTARTUP_NOSOCKET)
    }

    /// `MFStartup(MF_VERSION, MFSTARTUP_FULL)`.
    pub fn startup_full() -> Result<Self> {
        Self::startup_with(MFSTARTUP_FULL)
    }

    fn startup_with(flags: u32) -> Result<Self> {
        // SAFETY: ingen pekere; parres med `MFShutdown` i `drop`.
        unsafe { MFStartup(MF_VERSION, flags) }?;
        Ok(Self(()))
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: parer den vellykkede `MFStartup` i `startup_with`.
        let _ = unsafe { MFShutdown() };
    }
}

// ── MMCSS ────────────────────────────────────────────────────────────────────
// https://learn.microsoft.com/en-us/windows/win32/api/avrt/nf-avrt-avsetmmthreadcharacteristicsw
// https://learn.microsoft.com/en-us/windows/win32/api/avrt/nf-avrt-avrevertmmthreadcharacteristics
// https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service
//
// Fra dokumentasjonen: oppgavenavnet må være en undernøkkel av
// HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Multimedia\SystemProfile\Tasks
// (standard: «Audio», «Capture», «Distribution», «Games», «Playback»,
// «Pro Audio», «Window Manager»); `TaskIndex` er 0 ved første kall; funksjonen
// gjelder TRÅDEN som kaller, og `AvRevertMmThreadCharacteristics` kalles når
// oppgaven er ferdig.

/// MMCSS-oppgaven for videofangst.
pub const MMCSS_TASK_CAPTURE: &str = "Capture";

/// Den kallende tråden er registrert hos MMCSS til vakten slippes. Vakten er
/// `!Send`: registreringen gjelder tråden som laget den, og skal oppheves der.
#[derive(Debug)]
#[must_use = "the MMCSS registration is reverted when the guard is dropped"]
pub struct MmcssRegistration {
    handle: HANDLE,
    task_index: u32,
    _not_send: PhantomData<*const ()>,
}

impl MmcssRegistration {
    /// Registrerer den kallende tråden for `task` (f.eks.
    /// [`MMCSS_TASK_CAPTURE`]). Feiler med `ERROR_INVALID_TASK_NAME` hvis
    /// oppgaven ikke finnes i registeret.
    pub fn register_current_thread(task: &str) -> Result<Self> {
        let name = HSTRING::from(task);
        let mut task_index: u32 = 0;
        // SAFETY: `name` lever gjennom kallet; `task_index` er 0 ved første
        // kall, slik dokumentasjonen krever.
        let handle = unsafe { AvSetMmThreadCharacteristicsW(&name, &mut task_index) }?;
        Ok(Self {
            handle,
            task_index,
            _not_send: PhantomData,
        })
    }

    /// Oppgaveindeksen MMCSS ga tråden.
    pub fn task_index(&self) -> u32 {
        self.task_index
    }
}

impl Drop for MmcssRegistration {
    fn drop(&mut self) {
        // SAFETY: håndtaket kom fra `AvSetMmThreadCharacteristicsW` på denne
        // tråden (vakten er `!Send`).
        let _ = unsafe { AvRevertMmThreadCharacteristics(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    //! Bufferlåsen testes mot en falsk `IMFMediaBuffer`, så testen ikke trenger
    //! `mfplat.dll` (Windows Server-løperne har ikke nødvendigvis
    //! Media Foundation). Oppstart og MMCSS trenger ekte OS-tjenester og bevises
    //! på riggen.

    use super::*;
    use std::cell::{Cell, RefCell};
    use windows::Win32::Media::MediaFoundation::IMFMediaBuffer_Impl;
    use windows_core::implement;

    #[implement(IMFMediaBuffer)]
    struct FakeBuffer {
        bytes: RefCell<Vec<u8>>,
        current: u32,
        locks: Cell<i32>,
    }

    impl IMFMediaBuffer_Impl for FakeBuffer_Impl {
        fn Lock(
            &self,
            ppbbuffer: *mut *mut u8,
            pcbmaxlength: *mut u32,
            pcbcurrentlength: *mut u32,
        ) -> Result<()> {
            self.locks.set(self.locks.get() + 1);
            let mut bytes = self.bytes.borrow_mut();
            unsafe {
                *ppbbuffer = bytes.as_mut_ptr();
                if !pcbmaxlength.is_null() {
                    *pcbmaxlength = bytes.len() as u32;
                }
                if !pcbcurrentlength.is_null() {
                    *pcbcurrentlength = self.current;
                }
            }
            Ok(())
        }

        fn Unlock(&self) -> Result<()> {
            self.locks.set(self.locks.get() - 1);
            Ok(())
        }

        fn GetCurrentLength(&self) -> Result<u32> {
            Ok(self.current)
        }

        fn SetCurrentLength(&self, _: u32) -> Result<()> {
            Ok(())
        }

        fn GetMaxLength(&self) -> Result<u32> {
            Ok(self.bytes.borrow().len() as u32)
        }
    }

    #[test]
    fn lock_gives_only_the_valid_bytes_and_unlocks_once_on_drop() {
        let fake = windows_core::ComObject::new(FakeBuffer {
            bytes: RefCell::new(vec![1, 2, 3, 4, 0, 0, 0, 0]),
            current: 4,
            locks: Cell::new(0),
        });
        let buffer: IMFMediaBuffer = fake.to_interface();

        {
            let locked = buffer.lock().unwrap();
            assert_eq!(&*locked, &[1, 2, 3, 4]);
            assert_eq!(fake.locks.get(), 1);
        }
        assert_eq!(fake.locks.get(), 0, "Unlock skal kalles nøyaktig én gang");
    }
}
