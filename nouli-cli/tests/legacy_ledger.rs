//! Pre-refactor frozen settings are deliberately incompatible. Historical stores
//! remain intact and can still be opened by the matching released binary.
#[path = "support/harness.rs"]
pub mod harness;
#[path = "support/loopback.rs"]
pub mod loopback;
use harness::Home;
use std::path::Path;

#[test]
fn historical_frozen_settings_report_fresh_store_guidance_without_inference_or_rewrite() {
    for version in ["0.1.0", "0.2.0"] {
        let home = Home::create();
        let server = loopback::Server::with_handler(harness::typesafe);
        home.configure(&server, "system-one", false);
        std::fs::create_dir_all(home.store()).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../fixtures/contracts/ledger-v{version}/ledger.sqlite3"
        ));
        let database = home.store().join("ledger.sqlite3");
        std::fs::copy(fixture, &database).unwrap();
        let before = std::fs::read(&database).unwrap();
        let listed = home.run(["sessions", "list", "--json"]);
        assert!(listed.status.success(), "{listed:?}");
        for args in [
            &["view", "Trip", "--format", "text"][..],
            &["sessions", "export", "Trip", "--json"],
        ] {
            let output = home.run(args);
            assert_eq!(output.status.code(), Some(6), "{version}: {output:?}");
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(text.contains("unsupported_frozen_settings"), "{text}");
            assert!(text.contains("--store DIR"), "{text}");
            assert_eq!(std::fs::read(&database).unwrap(), before);
        }
        assert_eq!(server.count(), 0);
    }
}
