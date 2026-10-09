//! Reproducible reducer microbenchmark, not a window/rendering benchmark.
use std::{hint::black_box, time::Instant};

use con_core::{
    program_status::{Incoming, State},
    terminal_status::{Activity, Progress, SurfaceStatus},
};

fn run(program: bool, updates: u32) -> f64 {
    let mut surface = SurfaceStatus::new(1);
    let now = Instant::now();
    let start = Instant::now();
    for step in 0..updates {
        let percent = black_box((step % 101) as u8);
        if program {
            black_box(
                surface
                    .observe_program_status(
                        black_box(Incoming {
                            state: State::Working,
                            id: "build",
                            kind: None,
                            progress: Some(percent),
                            app: None,
                            title: None,
                            message: None,
                        }),
                        now,
                    )
                    .unwrap(),
            );
        } else {
            surface.observe_progress(black_box(Some(Progress {
                activity: Activity::Busy,
                percent: Some(percent),
            })));
        }
        black_box(surface.status(black_box(now)));
    }
    let elapsed = start.elapsed().as_secs_f64();
    assert_eq!(
        surface.status(now).unwrap().percent,
        Some(((updates - 1) % 101) as u8)
    );
    assert_eq!(surface.program_records().len(), usize::from(program));
    black_box(surface);
    elapsed * 1e9 / f64::from(updates)
}

fn main() {
    const UPDATES: u32 = 100_000;
    run(false, UPDATES);
    run(true, UPDATES);
    let mut progress = Vec::new();
    let mut program = Vec::new();
    for sample in 0..10 {
        // Alternate order to avoid always giving one condition the warm CPU.
        if sample % 2 == 0 {
            progress.push(run(false, UPDATES));
            program.push(run(true, UPDATES));
        } else {
            program.push(run(true, UPDATES));
            progress.push(run(false, UPDATES));
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "benchmark": "surface-status-reducer", "updates_per_sample": UPDATES,
            "progress_ns_per_update": progress, "program_ns_per_update": program,
            "scope": "In-memory update + status observation, no detail formatting, PTY, UI or paint"
        })
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn both_workloads_expose_the_last_progress_and_bound_storage() {
        assert!(super::run(false, 102).is_finite());
        assert!(super::run(true, 102).is_finite());
    }
}
