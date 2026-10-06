//! Registry path mapping and REG_SZ/REG_EXPAND_SZ decoding tests.
//!
//! These exercise the platform-neutral helpers backing the Windows
//! registry view, so the exact hive mapping and UTF-16 decoding
//! semantics are verified on every CI platform.

use coresight_apps::{
    decode_registry_string, split_hive_path, RegistryHive, RegistryValue, RegistryView,
    Win32RegistryView,
};

fn utf16_le(s: &str, with_nul: bool) -> Vec<u8> {
    let mut units: Vec<u16> = s.encode_utf16().collect();
    if with_nul {
        units.push(0);
    }
    units.into_iter().flat_map(u16::to_le_bytes).collect()
}

// ---- STEP 4: hive/path mapping -------------------------------------------

#[test]
fn hklm_paths_map_to_hklm_hive() {
    assert_eq!(
        split_hive_path("HKLM\\SOFTWARE\\Microsoft"),
        Some((RegistryHive::Hklm, "SOFTWARE\\Microsoft"))
    );
}

#[test]
fn hkcu_paths_map_to_hkcu_hive() {
    assert_eq!(
        split_hive_path("HKCU\\SOFTWARE\\Microsoft"),
        Some((RegistryHive::Hkcu, "SOFTWARE\\Microsoft"))
    );
}

#[test]
fn bare_hive_prefix_with_empty_subkey_maps() {
    assert_eq!(split_hive_path("HKLM\\"), Some((RegistryHive::Hklm, "")));
    assert_eq!(split_hive_path("HKCU\\"), Some((RegistryHive::Hkcu, "")));
}

#[test]
fn unknown_or_malformed_roots_are_rejected() {
    assert_eq!(split_hive_path("HKCR\\Foo"), None);
    assert_eq!(split_hive_path("HKLM"), None); // no separator
    assert_eq!(split_hive_path("HKCU"), None);
    assert_eq!(split_hive_path(""), None);
    assert_eq!(split_hive_path("Software\\App"), None);
    // Case-sensitive, as before: only exact documented roots match.
    assert_eq!(split_hive_path("hklm\\SOFTWARE"), None);
    assert_eq!(split_hive_path("Hklm\\SOFTWARE"), None);
}

#[test]
fn subkey_content_is_preserved_exactly() {
    let (_, rest) = split_hive_path("HKLM\\A\\\\B\\C").unwrap();
    assert_eq!(rest, "A\\\\B\\C");
}

// ---- STEP 5: REG_SZ / REG_EXPAND_SZ decoding -----------------------------

#[test]
fn decodes_valid_utf16_strings() {
    let bytes = utf16_le("Hello World", true);
    assert_eq!(decode_registry_string(&bytes), "Hello World");
}

#[test]
fn decodes_non_ascii_utf16() {
    let bytes = utf16_le("Ünïcode — 日本語", true);
    assert_eq!(decode_registry_string(&bytes), "Ünïcode — 日本語");
}

#[test]
fn empty_data_decodes_to_empty_string() {
    assert_eq!(decode_registry_string(&[]), "");
}

#[test]
fn all_nul_data_decodes_to_empty_string() {
    assert_eq!(decode_registry_string(&[0, 0]), "");
    assert_eq!(decode_registry_string(&[0, 0, 0, 0]), "");
}

#[test]
fn trailing_nul_is_terminator_not_content() {
    let mut bytes = utf16_le("App", true);
    bytes.extend_from_slice(&[0, 0]); // extra NUL units also trimmed
    assert_eq!(decode_registry_string(&bytes), "App");
}

#[test]
fn embedded_nul_is_preserved() {
    let bytes: Vec<u8> = [b'A' as u16, 0, b'B' as u16]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(decode_registry_string(&bytes), "A\u{0}B");
}

#[test]
fn odd_length_data_ignores_trailing_byte() {
    let mut bytes = utf16_le("Ok", false);
    bytes.push(0xAB); // malformed: single dangling byte
    assert_eq!(decode_registry_string(&bytes), "Ok");
}

#[test]
fn single_odd_byte_decodes_to_empty() {
    assert_eq!(decode_registry_string(&[0x41]), "");
}

#[test]
fn unpaired_surrogate_becomes_replacement_char() {
    // 0xD800 alone is an unpaired high surrogate: lossy decode must
    // substitute U+FFFD rather than fabricating a character.
    let bytes: Vec<u8> = 0xD800u16.to_le_bytes().to_vec();
    assert_eq!(decode_registry_string(&bytes), "\u{FFFD}");
}

// ---- STEP 3: Default availability across platforms -----------------------

#[test]
fn registry_view_has_default_on_every_platform() {
    // Both the Windows implementation and the non-Windows stub expose
    // `Default`; on non-Windows the absence of a registry degrades to
    // empty subkeys rather than fabricating entries. The generic bound
    // is what proves the trait impl exists (a direct `::default()` call
    // on a unit struct would be lint-flagged as meaningless).
    fn via_default<T: Default>() -> T {
        T::default()
    }
    let view: Win32RegistryView = via_default();
    assert!(view
        .subkeys("HKLM\\SOFTWARE\\Definitely\\Not\\A\\Real\\Key\\CoreSightTest")
        .is_empty());
    assert!(view
        .get_value(
            "HKLM\\SOFTWARE\\Definitely\\Not\\A\\Real\\Key\\CoreSightTest",
            "DisplayName"
        )
        .is_none());
}

#[cfg(not(windows))]
#[test]
fn non_windows_view_never_fabricates_values() {
    let view = Win32RegistryView::new();
    assert!(view.subkeys("HKLM\\SOFTWARE").is_empty());
    assert!(view.get_value("HKCU\\SOFTWARE", "Anything").is_none());
}

#[test]
fn registry_value_variants_round_trip() {
    // The decode helpers feed `RegistryValue::Sz`/`ExpandSz`; ensure the
    // value types keep their payloads exactly.
    let sz = RegistryValue::Sz(decode_registry_string(&utf16_le("Path\\To\\App", true)));
    assert_eq!(sz, RegistryValue::Sz("Path\\To\\App".to_string()));
    let expand = RegistryValue::ExpandSz(decode_registry_string(&utf16_le("%SystemRoot%", true)));
    assert_eq!(expand, RegistryValue::ExpandSz("%SystemRoot%".to_string()));
}
