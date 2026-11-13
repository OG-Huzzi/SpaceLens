//! Startup parsing tests.
//!
//! Synthetic text only: no plist is read, no registry is opened, and no unit
//! file is inspected. The tests prove the extraction rules, the rejection
//! reasons, the privilege mapping, and deterministic ordering.

use coresight_collect::{parse_startup, LaunchdScope, RawStartupEntry};
use coresight_systools::startup::{analyze_startup, StartupOptions, StartupSource, StartupTier};

fn plist(label: &str, program: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{program}</string>
        <string>--foreground</string>
    </array>
</dict>
</plist>"#
    )
}

fn entry(label: &str, program: &str, scope: LaunchdScope) -> RawStartupEntry {
    RawStartupEntry::LaunchdPlist {
        label: format!("{label}.plist"),
        plist_text: plist(label, program),
        scope,
    }
}

#[test]
fn launchd_user_agent_extracts_label_and_program_arguments() {
    let result = parse_startup(&[entry(
        "com.example.agent",
        "/usr/local/bin/example-agent",
        LaunchdScope::UserAgent,
    )]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    assert_eq!(item.name, "com.example.agent");
    assert_eq!(
        item.command.as_deref(),
        Some("/usr/local/bin/example-agent --foreground")
    );
    assert_eq!(
        item.source,
        StartupSource::LaunchAgent {
            label: "com.example.agent".to_string()
        }
    );
    assert!(item.enabled);

    let report = analyze_startup(&result.items, &StartupOptions::all());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].tier, StartupTier::ReviewFirst);
}

#[test]
fn launchd_system_daemon_is_off_limits_after_analysis() {
    let result = parse_startup(&[entry(
        "com.example.daemon",
        "/usr/local/libexec/example-daemon",
        LaunchdScope::SystemDaemon,
    )]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    assert_eq!(
        result.items[0].source,
        StartupSource::LaunchDaemon {
            label: "com.example.daemon".to_string()
        }
    );

    let report = analyze_startup(&result.items, &StartupOptions::all());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].tier, StartupTier::OffLimits);
    assert!(report.items[0].why.contains("launch daemon"));
}

#[test]
fn launchd_program_is_used_when_program_arguments_are_absent() {
    let text = r#"<plist version="1.0"><dict>
        <key>Label</key><string>com.example.simple</string>
        <key>Program</key><string>/usr/bin/true</string>
    </dict></plist>"#;
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "simplified".to_string(),
        plist_text: text.to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].command.as_deref(), Some("/usr/bin/true"));
}

#[test]
fn launchd_plist_without_label_is_rejected_with_reason() {
    let text = r#"<plist version="1.0"><dict>
        <key>ProgramArguments</key><array><string>/usr/bin/true</string></array>
    </dict></plist>"#;
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "missing-label".to_string(),
        plist_text: text.to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.items.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].source.contains("missing-label"));
    assert!(result.rejected[0].reason.contains("Label"));
    assert_eq!(result.unit_parse_failures, 0);
}

#[test]
fn launchd_plist_without_program_is_rejected_with_reason() {
    let text = r#"<plist version="1.0"><dict>
        <key>Label</key><string>com.example.empty</string>
        <key>ProgramArguments</key><array></array>
    </dict></plist>"#;
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "empty-program".to_string(),
        plist_text: text.to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.items.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("program"));
}

#[test]
fn launchd_binary_plist_is_rejected_not_decoded() {
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "binary.plist".to_string(),
        plist_text: "bplist00\u{0}\u{1}\u{2}".to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.items.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("binary"));
}

#[test]
fn launchd_commented_out_label_is_ignored() {
    let text = r#"<plist version="1.0"><dict>
        <!-- <key>Label</key><string>com.example.decoy</string> -->
        <key>Label</key><string>com.example.real</string>
        <key>ProgramArguments</key><array><string>/usr/bin/true</string></array>
    </dict></plist>"#;
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "comments".to_string(),
        plist_text: text.to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].name, "com.example.real");
}

#[test]
fn launchd_xml_entities_are_decoded() {
    let text = r#"<plist version="1.0"><dict>
        <key>Label</key><string>com.example.a&amp;b</string>
        <key>ProgramArguments</key>
        <array><string>/opt/A &amp; B/agent</string></array>
    </dict></plist>"#;
    let result = parse_startup(&[RawStartupEntry::LaunchdPlist {
        label: "entities".to_string(),
        plist_text: text.to_string(),
        scope: LaunchdScope::UserAgent,
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items[0].name, "com.example.a&b");
    assert_eq!(result.items[0].command.as_deref(), Some("/opt/A & B/agent"));
}

#[test]
fn registry_run_preserves_hive_and_key_path_exactly() {
    let hive = "HKCU".to_string();
    let key_path = "Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string();
    let result = parse_startup(&[RawStartupEntry::RegistryRunValue {
        hive: hive.clone(),
        key_path: key_path.clone(),
        value_name: "Example Agent".to_string(),
        command: "\"C:\\Program Files\\Example\\agent.exe\" --login".to_string(),
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    let item = &result.items[0];
    match &item.source {
        StartupSource::RegistryRun {
            hive: stored_hive,
            key_path: stored_key_path,
        } => {
            assert_eq!(stored_hive, &hive);
            assert_eq!(stored_key_path, &key_path);
        }
        other => panic!("expected RegistryRun, got {other:?}"),
    }
    assert_eq!(item.name, "Example Agent");
    assert_eq!(
        item.command.as_deref(),
        Some("\"C:\\Program Files\\Example\\agent.exe\" --login")
    );

    let report = analyze_startup(&result.items, &StartupOptions::all());
    assert_eq!(report.items[0].tier, StartupTier::ReviewFirst);
}

#[test]
fn registry_run_once_keeps_its_exact_key_path_and_is_privileged_under_hklm() {
    let key_path = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\RunOnce\\".to_string();
    let result = parse_startup(&[RawStartupEntry::RegistryRunValue {
        hive: "HKLM".to_string(),
        key_path: key_path.clone(),
        value_name: "Machine Setup".to_string(),
        command: "C:\\Setup\\once.exe".to_string(),
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    match &result.items[0].source {
        StartupSource::RegistryRunOnce {
            hive,
            key_path: stored,
        } => {
            assert_eq!(hive, "HKLM");
            assert_eq!(stored, &key_path);
        }
        other => panic!("expected RegistryRunOnce, got {other:?}"),
    }

    let report = analyze_startup(&result.items, &StartupOptions::all());
    assert_eq!(report.items[0].tier, StartupTier::OffLimits);
}

#[test]
fn registry_value_under_a_non_run_key_is_rejected() {
    let result = parse_startup(&[RawStartupEntry::RegistryRunValue {
        hive: "HKCU".to_string(),
        key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\RunServices".to_string(),
        value_name: "NotARunValue".to_string(),
        command: "C:\\Elsewhere\\thing.exe".to_string(),
    }]);

    assert!(result.items.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("Run or RunOnce"));
}

#[test]
fn systemd_unit_extracts_exec_start() {
    let text = r#"[Unit]
Description=Example backup

[Service]
Type=oneshot
ExecStart=/usr/local/bin/backup --now
"#;
    let result = parse_startup(&[RawStartupEntry::SystemdUnit {
        unit_name: "backup.service".to_string(),
        unit_text: text.to_string(),
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    assert_eq!(
        result.items[0].command.as_deref(),
        Some("/usr/local/bin/backup --now")
    );
    assert_eq!(
        result.items[0].source,
        StartupSource::SystemdUnit {
            unit: "backup.service".to_string()
        }
    );
}

#[test]
fn systemd_target_without_exec_start_is_rejected_and_counted() {
    let target = RawStartupEntry::SystemdUnit {
        unit_name: "graphical.target".to_string(),
        unit_text:
            "[Unit]\nDescription=Graphical target\n\n[Install]\nWantedBy=multi-user.target\n"
                .to_string(),
    };
    let plist_reject = RawStartupEntry::LaunchdPlist {
        label: "bad-plist".to_string(),
        plist_text: "not a plist".to_string(),
        scope: LaunchdScope::UserAgent,
    };
    let result = parse_startup(&[target, plist_reject]);

    assert!(result.items.is_empty());
    assert_eq!(result.rejected.len(), 2);
    assert_eq!(result.unit_parse_failures, 1);
    let target_reject = result
        .rejected
        .iter()
        .find(|reject| reject.source.contains("graphical.target"));
    match target_reject {
        Some(reject) => assert!(reject.reason.contains("ExecStart")),
        None => panic!("expected a reject for graphical.target"),
    }
}

#[test]
fn systemd_exec_start_joins_backslash_continuations() {
    let text = "[Service]\nExecStart=/usr/bin/example \\\n    --flag value\n";
    let result = parse_startup(&[RawStartupEntry::SystemdUnit {
        unit_name: "continued.service".to_string(),
        unit_text: text.to_string(),
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(
        result.items[0].command.as_deref(),
        Some("/usr/bin/example --flag value")
    );
}

#[test]
fn shell_profile_line_is_never_safe_to_disable() {
    let result = parse_startup(&[RawStartupEntry::ShellProfileLine {
        profile: "~/.zshrc".to_string(),
        line: "eval \"$(ssh-agent -s)\"".to_string(),
    }]);

    assert!(result.rejected.is_empty());
    assert_eq!(result.items.len(), 1);
    assert!(matches!(
        result.items[0].source,
        StartupSource::Unknown { .. }
    ));

    let report = analyze_startup(&result.items, &StartupOptions::all());
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].tier, StartupTier::OffLimits);
    assert_ne!(report.items[0].tier, StartupTier::SafeToDisable);
    assert!(report.items[0].why.contains("could not classify"));
}

#[test]
fn startup_output_is_deterministic_for_permuted_input() {
    let mut entries = vec![
        entry("com.example.zeta", "/usr/bin/zeta", LaunchdScope::UserAgent),
        entry(
            "com.example.alpha",
            "/usr/bin/alpha",
            LaunchdScope::SystemDaemon,
        ),
        RawStartupEntry::RegistryRunValue {
            hive: "HKCU".to_string(),
            key_path: "Software\\Microsoft\\Windows\\CurrentVersion\\Run".to_string(),
            value_name: "Beta".to_string(),
            command: "C:\\Beta\\beta.exe".to_string(),
        },
        RawStartupEntry::SystemdUnit {
            unit_name: "beta.service".to_string(),
            unit_text: "[Service]\nExecStart=/usr/bin/beta\n".to_string(),
        },
        RawStartupEntry::ShellProfileLine {
            profile: "~/.profile".to_string(),
            line: "/usr/local/bin/login-helper --quiet".to_string(),
        },
        RawStartupEntry::LaunchdPlist {
            label: "broken".to_string(),
            plist_text: "<plist><dict></dict></plist>".to_string(),
            scope: LaunchdScope::UserAgent,
        },
    ];

    let forward = parse_startup(&entries);
    entries.reverse();
    let backward = parse_startup(&entries);

    assert_eq!(forward, backward);
    assert_eq!(forward.items.len(), 5);
    assert_eq!(forward.rejected.len(), 1);
    assert_eq!(forward.unit_parse_failures, 0);
}

#[test]
fn empty_startup_inventory_is_an_empty_result() {
    let result = parse_startup(&[]);
    assert!(result.items.is_empty());
    assert!(result.rejected.is_empty());
    assert_eq!(result.unit_parse_failures, 0);
}
