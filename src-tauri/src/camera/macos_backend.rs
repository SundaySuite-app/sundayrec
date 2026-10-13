//! The macOS half of the common camera API (C1c): [`super::list`],
//! [`super::formats`] and [`super::start`] over [`super::avfoundation`].

use std::sync::Arc;

use sundayrec_core::camera_format::{classify_device, fourcc, DeviceClass};

use super::avfoundation::{self, CameraError as MacError, DeviceKind};
use super::queue::FrameQueue;
use super::{
    pixel_format_of_fourcc, BackendCounters, BackendHandle, CameraError, CameraInfo, CameraKind,
    Frame, FrameData, VideoFormat,
};

/// The common kind of an AVFoundation device. `External` is everything that is
/// not the Mac's own or a phone — a webcam, a capture card or a virtual camera —
/// and the shared classifier tells those apart from name and transport.
fn kind_of(device: &avfoundation::CameraDevice) -> CameraKind {
    match device.kind {
        DeviceKind::BuiltIn => CameraKind::BuiltIn,
        DeviceKind::Continuity => CameraKind::Continuity,
        DeviceKind::DeskView => CameraKind::DeskView,
        DeviceKind::External => {
            let model = device.usb_model.as_ref().map(|m| m.to_string());
            match classify_device(&device.name, model.as_deref(), Some(&device.transport)) {
                DeviceClass::Camera => CameraKind::Camera,
                DeviceClass::CaptureCard => CameraKind::CaptureCard,
                DeviceClass::Virtual => CameraKind::Virtual,
            }
        }
    }
}

pub(super) fn list() -> Vec<CameraInfo> {
    avfoundation::list_devices()
        .into_iter()
        .map(|d| CameraInfo {
            kind: kind_of(&d),
            model_id: d.usb_model.as_ref().map(|m| m.to_string()),
            id: d.unique_id,
            name: d.name,
        })
        .collect()
}

fn map_err(e: MacError) -> CameraError {
    match e {
        MacError::NotAuthorized(status) => CameraError::NotAuthorized(format!("{status:?}")),
        MacError::DeviceNotFound(id) => CameraError::DeviceNotFound(id),
        MacError::FormatNotFound(f) => CameraError::FormatNotFound(f),
        other => CameraError::Backend(other.to_string()),
    }
}

pub(super) fn formats(camera_id: &str) -> Result<Vec<VideoFormat>, CameraError> {
    Ok(avfoundation::formats(camera_id)
        .map_err(map_err)?
        .into_iter()
        .map(|f| VideoFormat {
            width: f.width,
            height: f.height,
            fps: f.fps,
            pixel_format: pixel_format_of_fourcc(f.pixel_format),
            native_code: fourcc(f.pixel_format),
            mac: f,
        })
        .collect())
}

struct MacSession {
    handle: avfoundation::CaptureHandle,
}

fn counters_of(stats: avfoundation::CaptureStats) -> BackendCounters {
    BackendCounters {
        dropped_by_os: stats.dropped_by_os,
        skipped: stats.without_image + stats.without_timestamp,
    }
}

impl BackendHandle for MacSession {
    fn describe(&self) -> String {
        format!("AVFoundation, {:?} mode", self.handle.mode())
    }

    fn counters(&self) -> BackendCounters {
        counters_of(self.handle.stats())
    }

    fn stop(self: Box<Self>) -> BackendCounters {
        counters_of(self.handle.stop())
    }
}

pub(super) fn start(
    camera_id: &str,
    format: &VideoFormat,
    queue: Arc<FrameQueue<Frame>>,
) -> Result<Box<dyn BackendHandle>, CameraError> {
    // The callback does ONE thing: wrap the frame and put it in the bounded
    // queue. `push` never waits for the consumer (see `queue`).
    let handle = avfoundation::start_with_fallback(camera_id, &format.mac, move |f| {
        queue.push(Frame {
            host_time_ns: f.host_time_ns,
            width: f.width,
            height: f.height,
            pixel_format: pixel_format_of_fourcc(f.pixel_format),
            bottom_up: false,
            data: FrameData::Native(f),
        });
    })
    .map_err(map_err)?;
    Ok(Box::new(MacSession { handle }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::camera_format::UsbModelId;

    fn dev(name: &str, kind: DeviceKind, transport: &str) -> avfoundation::CameraDevice {
        avfoundation::CameraDevice {
            unique_id: "id".into(),
            name: name.into(),
            kind,
            usb_model: (transport == "usb ").then(|| UsbModelId {
                vid: "046d".into(),
                pid: "082d".into(),
            }),
            transport: transport.into(),
        }
    }

    #[test]
    fn builtin_and_phone_keep_their_kind_whatever_their_name() {
        assert_eq!(
            kind_of(&dev("FaceTime HD Camera", DeviceKind::BuiltIn, "bltn")),
            CameraKind::BuiltIn
        );
        assert_eq!(
            kind_of(&dev("Richards iPhone", DeviceKind::Continuity, "usb ")),
            CameraKind::Continuity
        );
        assert_eq!(
            kind_of(&dev("Desk View", DeviceKind::DeskView, "usb ")),
            CameraKind::DeskView
        );
    }

    #[test]
    fn external_devices_split_into_camera_card_and_virtual() {
        assert_eq!(
            kind_of(&dev("Logitech BRIO", DeviceKind::External, "usb ")),
            CameraKind::Camera
        );
        assert_eq!(
            kind_of(&dev("Cam Link 4K", DeviceKind::External, "usb ")),
            CameraKind::CaptureCard
        );
        assert_eq!(
            kind_of(&dev("OBS Virtual Camera", DeviceKind::External, "virt")),
            CameraKind::Virtual
        );
    }
}
