use super::*;
use crate::{Joint, JointTransform, Skeleton};

#[test]
fn nearest_winding_and_cached_patch_have_bounded_continuity() {
    let positions = [
        Vec3::new(-0.1, -0.1, 0.0),
        Vec3::new(0.1, -0.1, 0.0),
        Vec3::new(0.0, 0.1, 0.0),
        Vec3::new(-0.1, -0.1, 0.003),
        Vec3::new(0.1, -0.1, 0.003),
        Vec3::new(0.0, 0.1, 0.003),
    ];
    let mut patches = [
        Patch {
            vertices: [0, 1, 2].map(SurfaceVertex::Tee),
            rest_center: Vec3::ZERO,
            source: [0, 1, 2],
            zone: Zone::Tee,
        },
        Patch {
            vertices: [3, 4, 5].map(SurfaceVertex::Tee),
            rest_center: Vec3::Z * 0.003,
            source: [3, 4, 5],
            zone: Zone::Tee,
        },
    ];
    let mut nodes = Vec::new();
    layout(&mut patches, 0, &mut nodes);
    let mut field = Field::new(patches.len(), nodes.len());
    field.refit(&nodes, &patches, |v| match v {
        SurfaceVertex::Tee(i) => positions[i],
        _ => unreachable!(),
    });
    let (contact, cached) = field.contact(&nodes, None, Vec3::Z * 0.001, None);
    assert_eq!(cached, Some(0));
    assert!((contact.depth - 0.005).abs() < 1.0e-7);
    assert_eq!(contact.normal, Vec3::Z);
    assert_eq!(field.contact(&nodes, None, Vec3::Z * 0.002, cached).1, Some(0));
    assert_eq!(field.contact(&nodes, None, Vec3::Z * 0.003, cached).1, Some(1));
    assert_eq!(field.contact(&nodes, None, Vec3::Z * 0.04, cached).1, None);
}

#[test]
fn weighted_link_jacobian_matches_a_finite_difference() {
    let links: [Link; LINKS] = std::array::from_fn(|i| Link {
        pivot: Vec3::new(0.01 * i as f32, -0.08 * i as f32, 0.015 * i as f32),
        parent: Quat::from_rotation_y(0.09 * i as f32),
    });
    let angles = [Vec3::new(0.1, 0.04, 0.07), Vec3::new(-0.09, 0.03, 0.02), Vec3::Z * 0.05, Vec3::X * 0.04];
    let normal = Vec3::new(0.6, -0.12, 0.8).normalize();
    let mut sample = [WeightedPoint { position: Vec3::ZERO, affected: None, weight: 0.0 }; 12];
    for (i, p) in sample.iter_mut().take(6).enumerate() {
        p.position = Vec3::new(i as f32 * 0.017, -0.42 + 0.012 * i as f32, -0.03);
        p.affected = (i < 5).then_some(i.min(3));
        p.weight = 1.0 / 6.0;
    }
    let placed = place_links(&links, angles);
    let (position, lever) = sample_position(sample, &placed);
    let measured = jacobian(Contact { normal, depth: 0.01 }, lever, &placed);
    for (i, derivative) in measured.into_iter().enumerate() {
        let mut nearby = angles;
        nearby[i / 3][i % 3] += 0.0005;
        let other = sample_position(sample, &place_links(&links, nearby)).0;
        let expected = normal.dot(other - position) / 0.0005;
        assert!((derivative - expected).abs() < 0.0003, "axis {i}: {derivative} vs {expected}");
    }
}

fn test_pose() -> CharacterPose {
    let joints: [Joint; 5] = std::array::from_fn(|i| Joint {
        name: format!("joint-{i}"),
        parent: i.checked_sub(1).map(|v| v as u16),
        rest: JointTransform {
            translation: [0.0, if i > 1 { -0.1 } else { 0.0 }, 0.0],
            ..JointTransform::IDENTITY
        },
    });
    let bytes = Skeleton::new(&joints).to_cooked().unwrap();
    CharacterPose::new(Skeleton::read(&bytes).unwrap(), &[], &[], Vec::new())
}

#[test]
fn full_apply_keeps_relief_across_frozen_lod_and_releases_at_fractional_time() {
    let mut sample = Sample { source: [0; 3], parts: [SamplePart::default(); 12] };
    sample.parts[0] = SamplePart { joint: 4, affected: Some(3), local: Vec3::NEG_Y * 0.1, weight: 1.0 };
    let lock = LockProfile { strand: 0, joints: [1, 2, 3, 4], samples: vec![sample] };
    let patches = vec![Patch {
        vertices: [0, 1, 2].map(SurfaceVertex::Tee),
        rest_center: Vec3::ZERO,
        source: [0, 1, 2],
        zone: Zone::Tee,
    }];
    let nodes = vec![Node { start: 0, end: 1, right: None }];
    let mut residual = Residual {
        profile: Profile { patches, nodes, leg_root: None, skin: Vec::new(), locks: vec![lock] },
        locks: vec![State::new(1)],
        fields: [Field::new(1, 1), Field::new(1, 1)],
        skin: Vec::new(),
        previous_skin: Vec::new(),
        history: false,
        distal: None,
    };
    let strands = [Strand {
        joints: vec![1, 2, 3, 4],
        samples: Vec::new(),
        side: 1.0,
        front: 1.0,
        shoulder_route: None,
        relief: Default::default(),
        previous_probes: Vec::new(),
    }];
    let close = [Vec3::new(-0.07, -0.50, 0.013), Vec3::new(0.07, -0.50, 0.013), Vec3::new(0.0, -0.30, 0.013)];
    let far = close.map(|p| p - Vec3::Z);
    let mut pose = test_pose();
    residual.apply(&mut pose, Affine3A::IDENTITY, &strands, &close, &close, None);
    assert_eq!(pose.locals()[1].rotation, Quat::IDENTITY);
    residual.apply(&mut pose, Affine3A::IDENTITY, &strands, &close, &close, Some(1.0));
    let drawn = residual.locks[0].angles;
    assert!(drawn.iter().any(|p| p.length() > 0.001));
    assert_eq!(pose.model()[1].translation, Affine3A::IDENTITY.translation);

    pose = test_pose();
    residual.sleep();
    residual.apply(&mut pose, Affine3A::IDENTITY, &strands, &far, &close, Some(0.0));
    for (joint, angle) in (1..=4).zip(drawn) {
        let target = Quat::from_scaled_axis(angle);
        assert!(pose.locals()[joint].rotation.dot(target).abs() > 0.999999);
    }
    pose = test_pose();
    residual.sleep();
    residual.apply(&mut pose, Affine3A::IDENTITY, &strands, &far, &far, Some(0.25));
    let released = residual.locks[0].angles;
    assert!(released.iter().zip(drawn).all(|(a, b)| a.length() <= b.length() + 1.0e-7));
    assert!(released.iter().zip(drawn).any(|(a, b)| a.length() < b.length() - 1.0e-6));

    pose = test_pose();
    residual.reset();
    residual.apply(&mut pose, Affine3A::IDENTITY, &strands, &far, &far, None);
    assert_eq!(pose.locals()[1].rotation, Quat::IDENTITY);
    assert_eq!(residual.locks[0].angles, [Vec3::ZERO; LINKS]);
}

#[test]
fn combined_projection_preserves_global_extent_and_root_supplement_after_rate_limit() {
    let axes = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        Vec3::new(0.55, 0.8, -0.2).normalize(),
        Vec3::new(-0.4, 0.3, 0.8).normalize(),
    ];
    for dt in [0.0, 0.125, 0.25, 1.0, 2.0] {
        for old_axis in axes {
            for arm_axis in axes {
                let prior_p8 = Quat::from_axis_angle(old_axis, 26.0_f32.to_radians());
                let p8_goal = Quat::from_axis_angle(arm_axis, 28.0_f32.to_radians());
                let current_p8 = limited_turn(prior_p8, p8_goal, dt * 4.5);
                let old_direction = Quat::from_axis_angle(-arm_axis, 4.0_f32.to_radians());
                let previous = limited_turn(Quat::IDENTITY, old_direction * prior_p8, 28.0);
                let parent = Quat::from_axis_angle(arm_axis, 0.45);
                for candidate_axis in axes {
                    let mut angles = [Vec3::ZERO; LINKS];
                    angles[0] = candidate_axis * 4.0_f32.to_radians();
                    let desired = angles.map(|a| -a);
                    let result = project(desired, angles, previous, parent, current_p8, dt);
                    let actual = total_root(parent, result, current_p8);
                    assert!(degrees_between(previous, actual) <= dt * 4.5 + 0.001);
                    assert!(degrees_between(Quat::IDENTITY, actual) <= 28.001);
                    assert!(degrees_between(current_p8, actual) <= 4.001);
                    assert!(result[0].length().to_degrees() <= 4.001);
                }
            }
        }
    }
}

/// The search passes over nodes and triangles it can tell are no nearer than the best so far, and must
/// still find what a scan of every triangle does: the nearest within reach, the lowest index of equals.
#[test]
fn the_search_finds_what_a_scan_of_every_triangle_does() {
    let mut seed = 0x2545_f491_u32;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    let positions: Vec<_> =
        (0..600).map(|_| Vec3::new(random(), random(), random() * 0.3) * 0.12 - Vec3::splat(0.06)).collect();
    let mut patches: Vec<_> = (0..200)
        .map(|i| Patch {
            vertices: [3 * i, 3 * i + 1, 3 * i + 2].map(SurfaceVertex::Tee),
            rest_center: (positions[3 * i] + positions[3 * i + 1] + positions[3 * i + 2]) / 3.0,
            source: [3 * i, 3 * i + 1, 3 * i + 2].map(|v| v as u32),
            zone: Zone::Tee,
        })
        .collect();
    let mut nodes = Vec::new();
    layout(&mut patches, 0, &mut nodes);
    let mut field = Field::new(patches.len(), nodes.len());
    field.refit(&nodes, &patches, |v| match v {
        SurfaceVertex::Tee(i) => positions[i],
        SurfaceVertex::Skin(_) => unreachable!(),
    });
    let mut found = 0;
    for _ in 0..4000 {
        let point = Vec3::new(random(), random(), random()) * 0.16 - Vec3::splat(0.08);
        let mut best = (FIND_RADIUS * FIND_RADIUS, None);
        field.search(&nodes, 0, point, &mut best);
        let mut scan = (FIND_RADIUS * FIND_RADIUS, None);
        for (i, triangle) in field.triangles.iter().enumerate() {
            let distance = triangle.distance(point).0;
            if distance < scan.0 {
                scan = (distance, Some(i));
            }
        }
        assert_eq!(best, scan, "at {point}");
        found += usize::from(scan.1.is_some());
    }
    assert!(found > 1000, "{found} points had a triangle within reach");
}
