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
    let mentions = extract_mentions("请 <@cifera-nexus> 确认");
    assert_eq!(mentions.len(), 1);
    assert_eq!(mentions[0].as_str(), "cifera-nexus");
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
fn test_empty_handler_ignored() {
    let mentions = extract_mentions("<@> 空的");
    assert!(mentions.is_empty());
}

#[test]
fn test_uppercase_ignored() {
    let mentions = extract_mentions("<@LEWIS> 大写");
    assert!(mentions.is_empty());
}

#[test]
fn test_system_reserved_ignored() {
    let mentions = extract_mentions("<@system> 保留字");
    assert!(mentions.is_empty());
}

#[test]
fn test_consecutive_hyphens_ignored() {
    let mentions = extract_mentions("<@foo--bar> 连续连字符");
    assert!(mentions.is_empty());
}

#[test]
fn test_unclosed_mention_ignored() {
    let mentions = extract_mentions("<@lewis 未闭合");
    assert!(mentions.is_empty());
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
