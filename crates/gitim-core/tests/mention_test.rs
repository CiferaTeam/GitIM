use gitim_core::mention::extract_mentions;

#[test]
fn test_single_mention() {
    let mentions = extract_mentions("<@lewis> 请看一下");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_multiple_mentions() {
    let mentions = extract_mentions("<@lewis> 和 <@nexus> 讨论一下");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].as_str(), "lewis");
    assert_eq!(mentions[1].as_str(), "nexus");
}

#[test]
fn test_mention_in_continuation() {
    let mentions = extract_mentions("第一行内容\n第二行 <@coder> 看看");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "coder");
}

#[test]
fn test_mention_with_hyphen() {
    let mentions = extract_mentions("请 <@code-reviewer> 确认");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "code-reviewer");
}

#[test]
fn test_duplicate_mention_dedup() {
    let mentions = extract_mentions("<@lewis> 和 <@lewis> 重复了");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_bare_at_ignored() {
    let mentions = extract_mentions("@lewis 不是协议级 mention");
    assert!(mentions.is_empty());
}

#[test]
fn test_invalid_mention_formats_ignored() {
    // empty handler
    assert!(extract_mentions("<@> 空的").is_empty());
    // uppercase handler
    assert!(extract_mentions("<@LEWIS> 大写").is_empty());
    // reserved word "system"
    assert!(extract_mentions("<@system> 保留字").is_empty());
    // consecutive hyphens
    assert!(extract_mentions("<@foo--bar> 连续连字符").is_empty());
    // unclosed mention (no closing `>`)
    assert!(extract_mentions("<@lewis 未闭合").is_empty());
}

#[test]
fn test_nested_mention() {
    let mentions = extract_mentions("<@<@lewis>> 嵌套");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "lewis");
}

#[test]
fn test_no_mentions() {
    let mentions = extract_mentions("普通消息，没有 mention");
    assert!(mentions.is_empty());
}

#[test]
fn test_mention_at_line_boundaries() {
    let mentions = extract_mentions("<@alice>\n<@bob>");
    assert_eq!(mentions.len(), 2);
    assert_eq!(mentions[0].as_str(), "alice");
    assert_eq!(mentions[1].as_str(), "bob");
}
