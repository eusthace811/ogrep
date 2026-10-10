//! Compile-time optional local-search diagnostics. Normal builds have no clocks
//! or thread-local state here. Unlike --stats this never changes matching work.

#[derive(Clone, Copy)]
pub enum Phase {
    Compile,
    Postings,
    Filter,
    Metadata,
    Read,
    Decode,
    Match,
    VerifyRender,
    Flush,
}

#[cfg(not(feature = "profile-search"))]
mod implementation {
    use super::Phase;
    pub struct Session;
    impl Session {
        #[inline(always)]
        pub fn start(_: std::time::Instant) -> Self {
            Self
        }
    }
    pub struct Span;
    impl Drop for Span {
        fn drop(&mut self) {}
    }
    #[inline(always)]
    pub fn span(_: Phase) -> Span {
        Span
    }
}

#[cfg(feature = "profile-search")]
mod implementation {
    use super::Phase;
    use std::cell::RefCell;
    use std::time::{Duration, Instant};

    struct Record {
        start: Instant,
        open: Duration,
        phases: [Duration; 9],
        calls: [u64; 9],
    }
    thread_local! { static RECORD: RefCell<Option<Record>> = const { RefCell::new(None) }; }

    pub struct Session(bool);
    impl Session {
        pub fn start(start: Instant) -> Self {
            if std::env::var_os("OGREP_PROFILE_LOCAL").is_none_or(|v| v != "1") {
                return Self(false);
            }
            RECORD.with_borrow_mut(|record| {
                assert!(record.is_none(), "nested profiling session");
                *record = Some(Record {
                    start,
                    open: start.elapsed(),
                    phases: [Duration::ZERO; 9],
                    calls: [0; 9],
                });
            });
            Self(true)
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            if !self.0 {
                return;
            }
            let record = RECORD.with_borrow_mut(Option::take).unwrap();
            let mut phases = serde_json::Map::new();
            let mut calls = serde_json::Map::new();
            phases.insert(
                "open_ms".into(),
                (record.open.as_secs_f64() * 1000.0).into(),
            );
            for (i, (name, value)) in [
                "compile",
                "postings",
                "filter",
                "metadata",
                "read",
                "decode",
                "match",
                "verify_render",
                "flush",
            ]
            .into_iter()
            .zip(record.phases)
            .enumerate()
            {
                phases.insert(format!("{name}_ms"), (value.as_secs_f64() * 1000.0).into());
                calls.insert(name.into(), record.calls[i].into());
            }
            eprintln!(
                "[profile-local] {}",
                serde_json::json!({
                    "elapsed_ms": record.start.elapsed().as_secs_f64() * 1000.0,
                    "phases_ms": phases,
                    "phase_calls": calls,
                })
            );
        }
    }

    pub struct Span {
        phase: Phase,
        start: Option<Instant>,
    }
    pub fn span(phase: Phase) -> Span {
        Span {
            phase,
            start: RECORD.with_borrow(|r| r.as_ref().map(|_| Instant::now())),
        }
    }
    impl Drop for Span {
        fn drop(&mut self) {
            if let Some(start) = self.start {
                let elapsed = start.elapsed();
                RECORD.with_borrow_mut(|r| {
                    if let Some(record) = r {
                        record.phases[self.phase as usize] += elapsed;
                        record.calls[self.phase as usize] += 1;
                    }
                });
            }
        }
    }
}

pub use implementation::{Session, Span, span};
