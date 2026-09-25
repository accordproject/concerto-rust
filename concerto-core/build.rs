//! Build script for `accordproject-concerto-core`.
//!
//! Its only job is the oracle harness's explicit opt-out
//! (`tests/oracle/main.rs`, "Locating the corpus"): when
//! `CONCERTO_ORACLE_SKIP=1` is set, it sets the `concerto_oracle_skip` cfg,
//! which marks `replays_the_oracle_corpus` `#[ignore]`, so `cargo test`
//! reports it as `ignored` rather than as a pass. Without the opt-out, a
//! missing corpus fails the test. The library itself does not read the cfg.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(concerto_oracle_skip)");
    println!("cargo::rerun-if-env-changed=CONCERTO_ORACLE_SKIP");
    if std::env::var("CONCERTO_ORACLE_SKIP").is_ok_and(|v| v == "1") {
        println!("cargo::rustc-cfg=concerto_oracle_skip");
    }
}
