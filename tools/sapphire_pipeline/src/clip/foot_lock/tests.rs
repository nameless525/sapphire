//! The pass's pieces on their own; `clip::retarget::tests` holds it on every shipped retarget.

use super::*;

#[test]
fn runs_are_each_stretch_of_planted_ticks() {
    let planted = [true, true, false, false, true, false, true];
    assert_eq!(runs(&planted), [(0, 1), (4, 4), (6, 6)]);
    assert_eq!(run_ends(&planted), [1, 1, 0, 0, 4, 0, 6]);
    assert!(runs(&[false; 3]).is_empty());
}

#[test]
fn a_correction_fades_eased_to_nothing_over_its_span() {
    assert_eq!(fade(0, 6), 1.0);
    assert_eq!(fade(3, 6), 0.5);
    assert_eq!(fade(6, 6), 0.0);
    assert_eq!(fade(9, 6), 0.0);
    assert!(fade(1, 6) > 0.9 && fade(5, 6) < 0.1, "eased at both ends");
}

/// A leg bent forward, its hip at 1 m: the ankle lands where it's sent, the thigh and shin keep their
/// lengths, and the knee stays forward of the line from hip to ankle.
#[test]
fn the_solve_puts_the_ankle_where_its_sent_and_keeps_the_leg_its_length() {
    let (hip, knee, ankle) = (Vec3::new(0.1, 1.0, 0.0), Vec3::new(0.1, 0.5, 0.1), Vec3::new(0.1, 0.0, 0.0));
    let lateral = Vec3::X;
    for wanted in [Vec3::new(0.1, 0.0, 0.05), Vec3::new(0.08, 0.02, -0.04), Vec3::new(0.15, 0.0, 0.0)] {
        let (swing, bend) = solve(hip, knee, ankle, wanted, lateral).unwrap();
        let moved_knee = hip + det::quat_rotate(swing, knee - hip);
        let moved_ankle = moved_knee + det::quat_rotate(det::quat_mul(swing, bend), ankle - knee);
        assert!(moved_ankle.distance(wanted) < 1e-5, "{wanted}: {moved_ankle}");
        assert!((moved_knee.distance(hip) - knee.distance(hip)).abs() < 1e-5);
        assert!((moved_ankle.distance(moved_knee) - ankle.distance(knee)).abs() < 1e-5);
        assert!(moved_knee.z > (hip.z + moved_ankle.z) * 0.5, "{wanted}: the knee bends back");
    }
}

/// A straight leg sent closer bends its knee forward, by the body's axis; one sent past its reach stops
/// short, at [`STRAIGHT`] of its length.
#[test]
fn a_straight_leg_bends_forward_and_reaches_no_further_than_it_can() {
    let (hip, knee, ankle) = (Vec3::new(0.1, 1.0, 0.0), Vec3::new(0.1, 0.5, 0.0), Vec3::new(0.1, 0.0, 0.0));
    // The character's left is +X: from the right hip to the left one.
    let lateral = Vec3::X;
    let (swing, bend) = solve(hip, knee, ankle, Vec3::new(0.1, 0.05, 0.0), lateral).unwrap();
    let moved_knee = hip + det::quat_rotate(swing, knee - hip);
    assert!(moved_knee.z > 0.05, "the knee comes forward: {moved_knee}");
    let _ = bend;

    let bent = Vec3::new(0.1, 0.5, 0.1);
    let (swing, bend) = solve(hip, bent, ankle, Vec3::new(0.1, -0.5, 0.0), lateral).unwrap();
    let moved_knee = hip + det::quat_rotate(swing, bent - hip);
    let moved_ankle = moved_knee + det::quat_rotate(det::quat_mul(swing, bend), ankle - bent);
    let length = bent.distance(hip) + ankle.distance(bent);
    assert!((moved_ankle.distance(hip) - length * STRAIGHT).abs() < 1e-4, "{moved_ankle}");
    assert!(moved_knee.z > 0.0, "still bent forward");
}
