mod common;

use common::TestProject;

#[test]
fn test_tldr_prints_quick_reference() {
    let project = TestProject::with_name("tldr");

    let output = project.run_rite(&["tldr"]);

    output.assert_success();
    let stdout = output.stdout_str();
    assert!(stdout.contains("QUICK REFERENCE"));
    assert!(stdout.contains("rite send general"));
    assert!(stdout.contains("rite inbox --mentions"));
    assert!(stdout.contains("rite wait --mentions"));
    assert!(stdout.contains("rite mark-read general"));
}

#[test]
fn test_root_help_includes_quick_reference() {
    let project = TestProject::with_name("help-tldr");

    let output = project.run_rite(&["--help"]);

    output.assert_success();
    let stdout = output.stdout_str();
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("tldr"));
    assert!(stdout.contains("QUICK REFERENCE"));
    assert!(stdout.contains("rite send @other-agent"));
}
