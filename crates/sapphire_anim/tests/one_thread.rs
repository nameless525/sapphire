//! `evaluate_all` on a compute pool of one worker (M5 C1): each character evaluated once, in a few jobs, with
//! the results a plain loop gives. Its own process, so it starts the job system with one worker; the unit
//! tests run on the machine's.

/// A character's work: a run of float operations that depends on nothing but its own number, so any
/// difference in order, sharing or count shows in its bits.
fn work(state: &mut (u32, u32, f32)) {
    let (index, visits, value) = state;
    *visits += 1;
    let mut x = *index as f32 * 0.37 + 1.0;
    for step in 0..200 {
        x = (x * 1.000_3 + step as f32 * 0.001).sqrt() + x.sin() * 0.5;
    }
    *value = x;
}

#[test]
fn one_worker_poses_as_a_plain_loop() {
    let system =
        sapphire_jobs::init(sapphire_jobs::JobConfig { compute_threads: Some(1), ..Default::default() });
    assert_eq!(system.compute_pool().thread_num(), 1, "the job system started here, with one worker");
    for count in [1, 2, 12, 15, 60] {
        let mut serial: Vec<(u32, u32, f32)> = (0..count).map(|index| (index, 0, 0.0)).collect();
        let mut jobs = serial.clone();
        serial.iter_mut().for_each(work);
        let made = sapphire_anim::evaluate_all(&mut jobs, work);
        assert!(made <= 4 && made as u32 <= count, "{count}: {made} jobs for a worker and the caller");
        for (alone, job) in serial.iter().zip(&jobs) {
            assert_eq!(job.1, 1, "{count}: {} evaluated {} times", job.0, job.1);
            assert_eq!(alone.2.to_bits(), job.2.to_bits(), "{count}: {}", job.0);
        }
    }
}
