//! Segment descriptors derived from the existing campaign map registry.
//!
//! Current rows preserve menu reset launches. They do not represent earned
//! progression: the save format omits live map entity/script state, and a
//! complete independent state has not yet been proven for any segment.

use psx_segments::{ConsumeResult, LaunchError, LaunchRequest, Progression, Segment};

/// Existing menu entry behavior, including authored chapter arrival handling.
#[derive(Clone, Copy)]
pub enum Spawn {
    MenuStart,
}

type Descriptor = Segment<'static, u16, Spawn, &'static crate::save::Checkpoint>;

#[optimize(size)]
/// Resolve a display row without storing a second map table in resident RAM.
pub fn descriptor(row: usize) -> Option<Descriptor> {
    let &room = crate::map_catalog::PLAY_ORDER.get(row)?;
    Some(Segment {
        id: room as u32,
        name: crate::map_catalog::MAPS[room as usize],
        load: room as u16,
        spawn: Spawn::MenuStart,
        progression: Progression::Unverified("Full playthrough state not verified"),
        diagnostic: true,
    })
}

#[optimize(size)]
/// Consume a local control request, returning the unchanged menu load target.
///
/// A successful diagnostic target still uses `menu_start` in the caller.
/// Earned requests fail closed until full state is verified. No guessed
/// checkpoint or equipment enters the restore path.
pub fn consume(request: &mut LaunchRequest) -> ConsumeResult<usize, core::convert::Infallible> {
    if *request == LaunchRequest::IDLE {
        return ConsumeResult::Idle;
    }
    let row = crate::map_catalog::PLAY_ORDER
        .iter()
        .position(|&room| room as u32 == request.segment_id);
    let Some(segment) = row.and_then(descriptor) else {
        // Keep version/mode rejection precedence identical to the SDK.
        return psx_segments::consume(
            request,
            &[] as &[Descriptor],
            |_, _, _, _| unreachable!(),
            |_, _| unreachable!(),
        );
    };
    psx_segments::consume(
        request,
        &[segment],
        |_, _, _, _| unreachable!("no verified progression entries"),
        |room, _| Ok(*room as usize),
    )
}

#[optimize(size)]
/// Start an explicitly labeled diagnostic reset from a display row.
pub fn diagnostic_room(row: usize) -> Result<usize, LaunchError> {
    let Some(segment) = descriptor(row) else {
        return Err(LaunchError::UnknownId);
    };
    let mut request = LaunchRequest {
        version: psx_segments::REQUEST_VERSION,
        segment_id: segment.id,
        mode: psx_segments::MODE_DIAGNOSTIC,
    };
    match consume(&mut request) {
        ConsumeResult::Applied(Ok(room)) => Ok(room),
        ConsumeResult::Rejected(error) => Err(error),
        ConsumeResult::Idle => unreachable!(),
        ConsumeResult::Applied(Err(never)) => match never {},
    }
}

/// Gate control mailbox. The host publishes only while the guest is paused,
/// after startup initialization and before resuming the menu loop. This is a
/// control request, separate from gameplay observation telemetry.
#[no_mangle]
pub static mut SEGMENT_LAUNCH_REQUEST: LaunchRequest = LaunchRequest::IDLE;

/// Last control result: 0 idle, 1 diagnostic accepted, 2 unsupported version,
/// 3 unknown mode, 4 duplicate ID, 5 unknown ID, 6 unverified earned state,
/// 7 invalid provenance, 8 diagnostic unavailable. Failed requests are consumed.
#[no_mangle]
pub static mut SEGMENT_LAUNCH_RESULT: u32 = 0;

#[optimize(size)]
/// Consume a host-published request exactly once while the title menu is active.
pub fn poll_control() -> Option<usize> {
    // The host pauses the guest for the whole write, so the request's words
    // cannot change during this read. Clear before validation or game callbacks.
    let mut pending =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!(SEGMENT_LAUNCH_REQUEST)) };
    if pending == LaunchRequest::IDLE {
        return None;
    }
    unsafe {
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!(SEGMENT_LAUNCH_REQUEST),
            LaunchRequest::IDLE,
        );
    }
    let (status, room) = match consume(&mut pending) {
        ConsumeResult::Idle => (0, None),
        ConsumeResult::Applied(Ok(room)) => (1, Some(room)),
        ConsumeResult::Applied(Err(never)) => match never {},
        ConsumeResult::Rejected(error) => (
            match error {
                LaunchError::Version => 2,
                LaunchError::Mode => 3,
                LaunchError::DuplicateId => 4,
                LaunchError::UnknownId => 5,
                LaunchError::Unverified => 6,
                LaunchError::Provenance => 7,
                LaunchError::DiagnosticUnavailable => 8,
            },
            None,
        ),
    };
    unsafe {
        core::ptr::write_volatile(core::ptr::addr_of_mut!(SEGMENT_LAUNCH_RESULT), status);
    }
    room
}

#[cfg(test)]
mod tests {
    use super::*;

    #[optimize(size)]
    #[test]
    fn every_catalog_row_preserves_its_map_target_and_rejects_earned_launch() {
        let mut seen = [false; crate::map_catalog::MAPS.len()];
        for (row, &room) in crate::map_catalog::PLAY_ORDER.iter().enumerate() {
            let segment = descriptor(row).unwrap();
            assert!(!seen[room as usize]);
            seen[room as usize] = true;
            assert_eq!(segment.id, room as u32);
            assert_eq!(segment.name, crate::map_catalog::MAPS[room as usize]);
            assert!(matches!(segment.spawn, Spawn::MenuStart));
            assert!(matches!(segment.progression, Progression::Unverified(_)));
            assert_eq!(diagnostic_room(row), Ok(room as usize));
            let mut request = LaunchRequest {
                version: psx_segments::REQUEST_VERSION,
                segment_id: room as u32,
                mode: psx_segments::MODE_PLAYTHROUGH,
            };
            assert_eq!(
                consume(&mut request),
                ConsumeResult::Rejected(LaunchError::Unverified)
            );
            assert_eq!(request, LaunchRequest::IDLE);
        }
        assert!(seen.iter().all(|seen| *seen));
        assert!(descriptor(usize::MAX).is_none());
        assert_eq!(diagnostic_room(usize::MAX), Err(LaunchError::UnknownId));
    }

    #[optimize(size)]
    #[test]
    fn idle_and_unknown_controls_have_no_launch_target() {
        let mut request = LaunchRequest::IDLE;
        assert_eq!(consume(&mut request), ConsumeResult::Idle);
        request = LaunchRequest {
            version: psx_segments::REQUEST_VERSION,
            segment_id: u32::MAX,
            mode: psx_segments::MODE_DIAGNOSTIC,
        };
        assert_eq!(
            consume(&mut request),
            ConsumeResult::Rejected(LaunchError::UnknownId)
        );
        assert_eq!(request, LaunchRequest::IDLE);
        request.version = 99;
        assert_eq!(
            consume(&mut request),
            ConsumeResult::Rejected(LaunchError::Version)
        );
    }

    #[optimize(size)]
    #[test]
    fn mailbox_acknowledges_and_consumes_each_request_once() {
        unsafe {
            SEGMENT_LAUNCH_REQUEST = LaunchRequest {
                version: psx_segments::REQUEST_VERSION,
                segment_id: crate::map_catalog::TRAINING_START_ROOM as u32,
                mode: psx_segments::MODE_PLAYTHROUGH,
            };
        }
        assert_eq!(poll_control(), None);
        assert_eq!(unsafe { SEGMENT_LAUNCH_RESULT }, 6);
        assert_eq!(unsafe { SEGMENT_LAUNCH_REQUEST }, LaunchRequest::IDLE);
        unsafe {
            SEGMENT_LAUNCH_REQUEST.mode = psx_segments::MODE_DIAGNOSTIC;
            SEGMENT_LAUNCH_REQUEST.version = psx_segments::REQUEST_VERSION;
            SEGMENT_LAUNCH_REQUEST.segment_id = crate::map_catalog::TRAINING_START_ROOM as u32;
        }
        assert_eq!(
            poll_control(),
            Some(crate::map_catalog::TRAINING_START_ROOM)
        );
        assert_eq!(unsafe { SEGMENT_LAUNCH_RESULT }, 1);
        assert_eq!(poll_control(), None);
        assert_eq!(unsafe { SEGMENT_LAUNCH_REQUEST }, LaunchRequest::IDLE);
    }
}
