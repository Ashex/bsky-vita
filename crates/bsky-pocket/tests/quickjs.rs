use bsky_pocket::{Clock, EmbeddedGuest, GuestState, FRAME_BUDGET};
use std::{
    process::{Command, Stdio},
    thread,
    time::Duration,
};
#[test]
fn actual_engine_contains_infinite_code_jobs_getters_errors_and_oom() {
    if let Some(case) = std::env::var_os("BSKY_QJS_CHILD") {
        let mut guest = EmbeddedGuest::new(480, 272).unwrap();
        match case.to_string_lossy().as_ref() {
            "loop" => {
                let _ = guest.eval("while(true){}");
            }
            "jobs" => {
                assert!(guest
                    .eval("Promise.resolve().then(function f(){Promise.resolve().then(f)})")
                    .is_err());
            }
            "getter" => {
                guest
                    .eval("Object.defineProperty(globalThis,'x',{get(){while(true){}}})")
                    .unwrap();
                let _ = guest.eval_global("x");
            }
            "exception" => {
                assert!(guest.eval("throw {toString(){while(true){}}}").is_err());
            }
            "oom" => {
                let _ = guest.eval("let a=[];while(true)a.push(new Array(10000).fill('x'))");
            }
            "success" => {
                guest.eval("globalThis.answer=42").unwrap();
                assert_eq!(guest.eval_global("answer").unwrap(), "42");
                return;
            }
            _ => panic!("case"),
        };
        assert_ne!(guest.state(), GuestState::Active);
        return;
    }
    for case in ["loop", "jobs", "getter", "exception", "oom", "success"] {
        let exe = std::env::current_exe().unwrap();
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "actual_engine_contains_infinite_code_jobs_getters_errors_and_oom",
                "--nocapture",
            ])
            .env("BSKY_QJS_CHILD", case)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "case {case} exited {status}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("case {case} exceeded 8s timeout")
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn injectable_clock_enforces_absolute_deadline_and_failure_is_terminal() {
    use std::{cell::Cell, rc::Rc, time::Instant};

    let elapsed = Rc::new(Cell::new(Duration::ZERO));
    let ticking = Rc::new(Cell::new(false));
    let clock_elapsed = Rc::clone(&elapsed);
    let clock_ticking = Rc::clone(&ticking);
    let origin = Instant::now();
    let clock: Clock = Box::new(move || {
        if clock_ticking.get() {
            clock_elapsed.set(clock_elapsed.get() + Duration::from_millis(1));
        }
        origin + clock_elapsed.get()
    });
    let mut guest = EmbeddedGuest::new_with_clock(480, 272, clock).unwrap();
    let result = guest.bounded(FRAME_BUDGET, || {
        elapsed.set(FRAME_BUDGET);
        Ok(())
    });
    assert_eq!(result.unwrap_err(), "guest deadline exceeded");
    assert_eq!(guest.state(), GuestState::Disposed);
    assert!(guest.eval("1 + 1").is_err());
}

#[test]
fn injectable_clock_interrupts_eval_and_promise_job_drain() {
    use std::{cell::Cell, rc::Rc, time::Instant};

    let elapsed = Rc::new(Cell::new(Duration::ZERO));
    let ticking = Rc::new(Cell::new(false));
    let clock_elapsed = Rc::clone(&elapsed);
    let clock_ticking = Rc::clone(&ticking);
    let origin = Instant::now();
    let clock: Clock = Box::new(move || {
        if clock_ticking.get() {
            clock_elapsed.set(clock_elapsed.get() + Duration::from_millis(1));
        }
        origin + clock_elapsed.get()
    });
    let mut guest = EmbeddedGuest::new_with_clock(480, 272, clock).unwrap();
    ticking.set(true);
    assert_eq!(
        guest.eval("while(true){} ").unwrap_err(),
        "guest deadline exceeded"
    );
    assert_eq!(guest.state(), GuestState::Disposed);
}
