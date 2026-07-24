//! `ProviderRegistry` behavior (ADR-0015): builtin set, custom registration,
//! replacement, and the unknown-name pre-run error.

use locode_core::{BuiltProvider, ProviderInit, ProviderRegistry};

fn init() -> ProviderInit {
    ProviderInit {
        session_id: "sess-test".to_string(),
        model: None,
    }
}

#[test]
fn builtin_names_in_order() {
    let registry = ProviderRegistry::builtin();
    assert_eq!(
        registry.names(),
        vec!["anthropic", "openai-responses", "mock"]
    );
}

#[test]
fn mock_builds_keyless() {
    let built = ProviderRegistry::builtin()
        .build("mock", &init())
        .expect("mock needs no env");
    assert_eq!(built.model, "mock-1");
}

#[test]
fn unknown_name_lists_available() {
    let err = ProviderRegistry::builtin()
        .build("no-such-wire", &init())
        .expect_err("unknown name must fail");
    let msg = err.to_string();
    assert!(msg.contains("no-such-wire"), "names the bad input: {msg}");
    assert!(
        msg.contains("anthropic, openai-responses, mock"),
        "lists the available set: {msg}"
    );
}

#[test]
fn custom_registration_and_replacement() {
    // A custom wire under a new name, built on the mock provider.
    let registry = ProviderRegistry::builtin().register("custom-wire", |init| {
        let mock = ProviderRegistry::builtin().build("mock", init)?;
        Ok(BuiltProvider {
            provider: mock.provider,
            model: "custom-model".to_string(),
        })
    });
    assert_eq!(
        registry.names(),
        vec!["anthropic", "openai-responses", "mock", "custom-wire"]
    );
    let built = registry.build("custom-wire", &init()).expect("registered");
    assert_eq!(built.model, "custom-model");

    // Re-registering an existing name replaces in place (order preserved).
    let replaced = registry.register("mock", |init| {
        let mock = ProviderRegistry::builtin().build("mock", init)?;
        Ok(BuiltProvider {
            provider: mock.provider,
            model: "mock-replaced".to_string(),
        })
    });
    assert_eq!(
        replaced.names(),
        vec!["anthropic", "openai-responses", "mock", "custom-wire"]
    );
    let built = replaced.build("mock", &init()).expect("replaced mock");
    assert_eq!(built.model, "mock-replaced");
}
