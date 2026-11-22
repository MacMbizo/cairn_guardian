//! Local-time dates and screenshot times. Its own test binary: the offset is process-wide.

use aegis_core::filing::classify::{ClassifyInput, DateSource, classify};
use aegis_core::filing::naming::proposed_name;
use aegis_core::filing::set_local_offset;

/// South Africa: UTC+2 all year.
fn sast(_utc_ms: i64) -> i32 {
    7200
}

fn input<'a>(name: &'a str, mtime: i64) -> ClassifyInput<'a> {
    ClassifyInput { name, ext: "png", text: None, exif_taken_at: None, mtime, in_atomic_unit: false, learned_orgs: &[] }
}

// 2024-01-02 23:30:00 UTC = 2024-01-03 01:30:00 in South Africa.
const LATE_UTC: i64 = 1_704_238_200_000;

#[test]
fn modified_date_is_local_and_screenshot_time_falls_back_to_local_mtime() {
    set_local_offset(sast);
    let c = classify(&input("photo.jpg", LATE_UTC));
    assert_eq!(c.date_source, DateSource::Modified);
    assert_eq!(c.doc_date.as_deref(), Some("2024-01-03"), "the local date, not the UTC one");

    // Date from the name, no time in it: the time comes from mtime, in local time.
    let c = classify(&input("Screenshot 2024-01-03.png", LATE_UTC));
    assert_eq!(
        proposed_name(&c, "Screenshot 2024-01-03", "png", LATE_UTC).as_deref(),
        Some("2024-01-03 01.30.00 Screenshot.png")
    );
}

#[test]
fn exif_date_is_wall_clock_already() {
    set_local_offset(sast);
    // EXIF holds the camera's wall clock, stored as if UTC: no offset is added.
    let mut i = input("IMG_1.jpg", 0);
    i.exif_taken_at = Some(LATE_UTC);
    assert_eq!(classify(&i).doc_date.as_deref(), Some("2024-01-02"));
}

#[test]
fn screenshot_with_only_a_modified_date_keeps_its_name() {
    set_local_offset(sast);
    let c = classify(&input("Screenshot.png", LATE_UTC));
    assert_eq!(c.date_source, DateSource::Modified);
    assert_eq!(proposed_name(&c, "Screenshot", "png", LATE_UTC), None);
}
