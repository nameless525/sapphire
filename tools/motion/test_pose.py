"""Regression tests for the authoring solver's joint and contact frames."""
import unittest

from pose import (
    Foot, Pose, QI, REST, add, angle_between, cross, forearm_twist, length, norm, qaxis, qrot,
    reach_chain, solve_leg, solve_right_arm, sub, to_euler, turn,
)


class PoseTests(unittest.TestCase):
    def assert_near(self, a, b, epsilon=1e-6):
        self.assertLess(length(sub(a, b)), epsilon, (a, b))

    def test_limb_reaches_and_both_segments_share_the_same_signed_hinge(self):
        thigh = (0, -0.35, -0.01)
        calf = (0, -0.41, -0.025)
        hinge = norm(cross(thigh, calf))
        target = (0.05, -0.61, 0.23)
        upper, lower, miss = reach_chain((0, 0, 0), thigh, calf, target, (0, 0, 1), hinge, (0, 0, 1))
        a, b = qrot(upper, thigh), qrot(lower, calf)
        solved_hinge = norm(cross(a, b))
        self.assert_near(add(a, b), target)
        self.assert_near(qrot(upper, hinge), solved_hinge)
        self.assert_near(qrot(lower, hinge), solved_hinge)
        self.assertAlmostEqual(miss, 0.0)

    def test_pole_can_cross_the_reach_axis_without_a_flip(self):
        start = (0, 0, 0)
        thigh = (0, -0.35, 0)
        calf = (0, -0.41, 0)
        target = (0, -0.7, 0)
        hinge, fallback = (1, 0, 0), (0, 0, 1)
        for x in (-0.001, 0, 0.001):
            upper, lower, _ = reach_chain(start, thigh, calf, target, (x, -1, 0), hinge, fallback)
            self.assertGreater(qrot(upper, thigh)[2], 0.1)
            self.assert_near(add(qrot(upper, thigh), qrot(lower, calf)), target)

    def test_heel_and_ball_pivots_hold_in_world_space(self):
        root, root_turn = (0.04, 0, 0.05), 25.0
        for side, x, pivot, pitch, toe in (
            ('Left', 0.19, 'heel', -18, None),
            ('Right', -0.10, 'ball', 35, -29),
        ):
            pose = Pose()
            pose.hips = (0, -0.10, 0)
            foot = Foot(x, 0.08, yaw=12, pitch=pitch, lift=0.01, toe=toe, pivot=pivot)
            miss = solve_leg(pose, side, foot, root, root_turn)
            contact = pose.sole(side)[int(pivot == 'heel')]
            self.assertAlmostEqual(miss, 0.0)
            world_contact = add(root, qrot(qaxis((0, 1, 0), root_turn), contact))
            self.assert_near(world_contact, (x, 0.01, 0.08))

    def test_grip_roll_keeps_the_wrist_and_blade_targets(self):
        pose = Pose().fk()
        shoulder = pose.pos['RightUpperArm']
        wrist = add(shoulder, (-0.11, -0.26, 0.19))
        elbow = (-0.45, -0.7, -0.45)
        solve_right_arm(pose, wrist, elbow)
        forearm = pose.rot['RightLowerArm']
        blade = norm((-0.7, -0.4, -0.6))
        solve_right_arm(pose, wrist, elbow, blade, (0, 1, 0))
        self.assert_near(pose.pos['RightHand'], wrist)
        self.assert_near(pose.blade_dir(), blade)
        bind = sub(REST['RightHand'], REST['RightLowerArm'])
        self.assert_near(qrot(pose.rot['RightLowerArm'], bind), qrot(forearm, bind))
        self.assertGreater(angle_between(qrot(pose.rot['RightLowerArm'], (0, 1, 0)), qrot(forearm, (0, 1, 0))), 5)

    def test_exact_gimbal_lock_round_trips_as_a_rotation(self):
        for pitch in (90, -90):
            original = turn(65, pitch, 40)
            recovered = turn(*to_euler(original))
            self.assertAlmostEqual(abs(sum(a * b for a, b in zip(original, recovered))), 1.0)

    def test_forearm_roll_does_not_flip_as_the_grip_reverses(self):
        axis = (1, 0, 0)
        up = (0, 1, 0)
        before = forearm_twist(QI, qaxis(axis, 179), axis)
        after = forearm_twist(QI, qaxis(axis, 181), axis)
        self.assertLess(angle_between(qrot(before, up), qrot(after, up)), 1.01)
