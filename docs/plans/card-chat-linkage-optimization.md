# Card-Chat Reference Preview And Navigation

> Goal: make GitIM references in chat structured, readable, previewable, and directly navigable.

## Product Contract

1. A reference points to a structured target: `channel`, optional `card_id`, and optional `line_number`.
2. Agents emit the reference id. The frontend resolves card titles from card metadata.
3. Optional `|label` is display fallback only.
4. Hovering a card or message reference opens a scrollable preview.
5. Clicking the reference or preview `Open` action performs the navigation jump.
6. Card detail exposes `#channel` as a deterministic return control and supports `?line=` for discussion-line highlighting.

## Canonical Schema

| Target | Text |
|--------|------|
| Channel | `<#channel>` |
| Channel message | `<#channel:L000042>` |
| Card | `<#channel/card-id>` |
| Card discussion message | `<#channel/card-id:L000004>` |
| Card fallback label | `<#channel/card-id|label>` |
| Card discussion fallback label | `<#channel/card-id:L000004|label>` |

Rules:

- `channel` follows the existing channel validator.
- `card-id` follows the existing core card id validator.
- Agents write zero-padded line markers. The frontend reader also accepts short legacy markers such as `L22` for card discussion links.
- Card titles are resolved from `card.meta.yaml` through frontend store/API reads.

## Reader Compatibility

The frontend reader supports these historical room forms:

- `#channel/card-id`
- `#channel/card-id L4`
- `<#channel/card-id:L22>`
- `card \`card-id\`` when the current channel has the matching card and nearby text signals a card reference
- `` `card-id` `` near `card`, `Card`, `卡`, or `卡片` under the same matching rule

## Implementation Map

Protocol:

- `crates/gitim-core/src/types/link.rs`: `LinkKind::Card`.
- `crates/gitim-core/src/link.rs`: canonical card/card-line/label parsing.
- `crates/gitim-daemon/src/handlers/serde.rs`: `kind: "card"` JSON.
- `crates/gitim-core/src/responses.rs`: `CreateCardResponse.ref`.
- `crates/gitim-daemon/src/card_handlers.rs`: create-card response emits `<#channel/card-id>`.
- `crates/gitim-cli/src/commands/card.rs`: human create output includes the canonical ref.

Prompt:

- `crates/gitim-agent-provider/src/prompts.rs`
- `crates/gitim-agent-provider/src/hermes/prompts.rs`
- `crates/gitim-agent-provider/tests/prompt_test.rs`

Frontend:

- `products/gitim/frontend/src/lib/message-parser.ts`: canonical and high-confidence legacy parsing.
- `products/gitim/frontend/src/components/chat/reference-preview.tsx`: card/message hover previews.
- `products/gitim/frontend/src/components/chat/message-body.tsx`: inline rendering, title resolution, legacy inline-code card upgrades.
- `products/gitim/frontend/src/components/cards/card-detail.tsx`: `?line=` highlighting and deterministic `#channel` return.
- `products/gitim/frontend/src/components/cards/card-create-dialog.tsx`: create success shows canonical ref.
- `products/gitim/frontend/src/daemon-web/handlers.ts`: browser runtime create-card response matches daemon schema.

## Preview Behavior

Card preview:

- Lazy-loads `readCard(channel, cardId)`.
- Shows card title, status, assignee, channel, full id, and discussion rows.
- For card-line refs, loads a small window around the target and highlights it.

Message preview:

- Uses current chat store when possible.
- Reads a small channel window when the target channel is not currently loaded.
- Highlights the target line inside the preview.

## Validation

Scoped checks:

```bash
cargo test -p gitim-core card
cargo test -p gitim-daemon serializes_card_link
cargo test -p gitim-daemon test_create_card_happy_path
cargo test -p gitim-agent-provider gitim_api_exposes_message_body_markers
npm --prefix products/gitim/frontend run test -- message-parser
npm --prefix products/gitim/frontend run lint
npm --prefix products/gitim/frontend run build
```

Final checks:

```bash
cargo test -p gitim-core
cargo test -p gitim-daemon
npm --prefix products/gitim/frontend run test
npm --prefix products/gitim/frontend run build
```
