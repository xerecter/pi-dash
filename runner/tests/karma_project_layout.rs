//! Contract tests for the Karma project workspace layout.
//!
//! Karma runs must be isolated below the project directory returned by
//! `karma project info`: one component for the runner and one for the work
//! item.  These tests intentionally exercise the small, pure path helpers so
//! they can run without a Karma installation or network access.

use std::path::Path;

use karma_team::karma::project::{operation_dir, slugify_component};

#[test]
fn slugify_component_uses_fallback_for_empty_or_whitespace_input() {
    assert_eq!(slugify_component("", "runner"), "runner");
    assert_eq!(slugify_component("   ", "runner"), "runner");
}

#[test]
fn slugify_component_cannot_emit_path_separators_or_parent_components() {
    let slug = slugify_component("../runner/../../work item", "fallback");

    assert!(
        !slug.is_empty(),
        "a usable fallback must always be returned"
    );
    assert!(
        !slug.contains('/'),
        "slash must not escape the project root"
    );
    assert!(
        !slug.contains('\\'),
        "backslash must not escape the project root"
    );
    assert_ne!(slug, ".", "dot is not a safe directory component");
    assert_ne!(slug, "..", "parent is not a safe directory component");
}

#[test]
fn operation_dir_is_exactly_runner_then_work_item_below_project_root() {
    let project_root = Path::new("/var/lib/karma/projects/example/main");
    let operation = operation_dir(project_root, "runner-01", "KTDEV/10");

    assert!(
        operation.starts_with(project_root),
        "operation directory escaped Karma project root: {}",
        operation.display()
    );

    let relative = operation
        .strip_prefix(project_root)
        .expect("operation must be below project root");
    let components: Vec<_> = relative.components().collect();
    assert_eq!(components.len(), 2, "expected <runner>/<work-item> layout");
    assert_eq!(components[0].as_os_str(), "runner-01");
    assert_ne!(
        components[1].as_os_str(),
        "..",
        "work item may not traverse up"
    );
}

#[test]
fn operation_dir_never_allows_absolute_or_traversal_inputs_to_replace_root() {
    let project_root = Path::new("/var/lib/karma/projects/example/main");
    for (runner, work_item) in [("../runner", "../item"), ("/tmp/runner", "/tmp/item")] {
        let operation = operation_dir(project_root, runner, work_item);
        assert!(
            operation.starts_with(project_root),
            "unsafe input escaped project root: {}",
            operation.display()
        );
    }
}
