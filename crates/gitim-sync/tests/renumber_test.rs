use gitim_sync::renumber::renumber_batch;

#[test]
fn test_renumber_simple() {
    let batch = "\
[L000003][P000000][@nexus][20250316T120000Z] new topic
[L000004][P000003][@lewis][20250316T120500Z] reply
";
    let result = renumber_batch(batch, 5).unwrap();
    assert!(result.contains("[L000006]"));
    assert!(result.contains("[L000007]"));
    assert!(result.contains("[P000000]"));
    assert!(result.contains("[P000006]"));
}

#[test]
fn test_renumber_preserves_external_refs() {
    let batch = "[L000003][P000002][@nexus][20250316T120000Z] reply to existing\n";
    let result = renumber_batch(batch, 5).unwrap();
    assert!(result.contains("[L000006]"));
    assert!(result.contains("[P000002]"));
}

#[test]
fn test_renumber_with_continuations() {
    let batch = "\
[L000003][P000000][@nexus][20250316T120000Z] multi
continuation line
[L000004][P000003][@lewis][20250316T120500Z] reply
";
    let result = renumber_batch(batch, 10).unwrap();
    assert!(result.contains("[L000011]"));
    assert!(result.contains("continuation line"));
    assert!(result.contains("[L000012]"));
    assert!(result.contains("[P000011]"));
}
