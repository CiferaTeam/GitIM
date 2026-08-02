# GitIM documentation experience research

## Research conclusion

GitIM documentation should lead with a reader goal or question, then reveal the product model through one concrete scenario. Concepts, diagrams, examples, and reference facts should support that journey at the point where they become useful.

A page should therefore have one dominant narrative. The current pattern of presenting many equally weighted concept cards gives every term the same visual priority and leaves the reader to infer the relationships. A stronger pattern is:

1. Establish the outcome and why it matters.
2. Show the whole system in one focused diagram.
3. Walk through one representative example.
4. Expand the major parts in the order encountered.
5. Link to precise reference material and the next useful task.

## Evidence from mature documentation systems

### Separate learning, doing, understanding, and lookup

Diátaxis defines four distinct documentation needs: tutorials, how-to guides, explanation, and reference. Tutorials teach through a meaningful, achievable activity; how-to guides help a competent user reach a practical goal; explanation connects ideas and provides context; reference describes the product precisely and consistently. Mixing these jobs makes a page harder to follow because each mode assumes a different reader intent. [Diátaxis overview](https://diataxis.fr/), [tutorials](https://diataxis.fr/tutorials/), [how-to guides](https://diataxis.fr/how-to-guides/), [explanation](https://diataxis.fr/explanation/), [reference](https://diataxis.fr/reference/)

GitHub applies a similar content model. Its concept guidance asks writers to explain what a feature is, why it is useful, and where it is used, while procedural content remains focused on completing a task. GitHub also gives articles a consistent order: title, concise intro, concept or reference context, prerequisites, procedure, troubleshooting, next steps, and focused further reading. [GitHub concepts content type](https://docs.github.com/en/contributing/style-guide-and-content-model/concepts-content-type), [contents of a GitHub Docs article](https://docs.github.com/en/contributing/style-guide-and-content-model/contents-of-a-github-docs-article)

**GitIM principle:** organize the documentation around reader intent. Keep `Quick start` as a guided learning journey; make common operations task-oriented; use concept pages for product mental models; keep protocol, CLI, API, and file schemas as reference.

### Reveal complexity progressively

Cloudflare's official writing guidance says to state the primary answer in the main flow, use disclosures only for supplementary information, and introduce technical concepts from basics to advanced material while explaining the “why” before the “how.” It also recommends providing information only when it is pertinent to the reader's current point in the journey. [Cloudflare writing guidelines](https://developers.cloudflare.com/style-guide/documentation-content-strategy/writing-guidelines/)

Google's illustration guidance turns the same principle into a visual pattern: begin with a simple big picture, divide a complex system into subsystems, then show each subsystem in a separate, more detailed illustration. [Google: Illustrating](https://developers.google.com/tech-writing/two/illustrations)

**GitIM principle:** use three visible levels:

- **Level 1 — orientation:** one sentence and one diagram that answer “what is happening?”
- **Level 2 — walkthrough:** one real scenario that follows the important objects and state changes.
- **Level 3 — detail:** focused sections for each major subsystem, with supplementary fields, edge cases, and implementation facts behind links or disclosures.

Every level should make sense on its own. A reader can stop after orientation, follow the example for working knowledge, or continue into detailed reference.

### Make diagrams carry a single teaching point

Google recommends writing the caption first, keeping a diagram to roughly one paragraph of information, and using callouts to focus attention. Complex systems should be split into a big-picture diagram and smaller subsystem views. [Google: Illustrating](https://developers.google.com/tech-writing/two/illustrations)

Cloudflare uses diagrams across all content types to show processes, architecture, and interactions. Its style guide favors scalable SVGs and editable, searchable Mermaid diagrams, with clear alternative text. [Cloudflare diagram guidance](https://developers.cloudflare.com/style-guide/documentation-content-strategy/component-attributes/diagrams/)

**GitIM principle:** every diagram must have:

- One takeaway stated in its caption.
- Three to five visually dominant objects.
- A clear direction of movement or ownership.
- Labels that remain understandable without color.
- Adjacent text describing the same essential relationship.

Use product-native diagrams rather than decorative illustrations: message routing, repository ownership, card lifecycle, flow execution, runtime topology, and distributed sync are the highest-value subjects.

### Teach with a worked example before cataloging terms

Diátaxis tutorials emphasize meaningful action, small steps, visible results, and an achievable goal. GitHub concept pages require use cases or examples, and Diátaxis reference guidance recommends concise usage examples to place factual material in context. [Diátaxis tutorials](https://diataxis.fr/tutorials/), [GitHub concepts content type](https://docs.github.com/en/contributing/style-guide-and-content-model/concepts-content-type), [Diátaxis reference](https://diataxis.fr/reference/)

Google's sample-code guidance recommends concise, correct examples with setup instructions and expected results, progressing from basic examples to more advanced uses. [Google: Creating sample code](https://developers.google.com/tech-writing/two/sample-code)

**GitIM principle:** each core page should carry one named scenario through the product. For example, the Messaging page can follow:

> Lewis asks `@planner` to turn a release goal into work. The message is written to a channel thread, routing selects the intended agent, the reply points back to its parent line, and the resulting Git commit preserves the exchange.

The page can then expand `Channel`, `Message`, `Recipient set`, `Reply chain`, and `Commit` exactly when each appears. Show the resulting UI state and the smallest relevant `.thread` excerpt or CLI response, followed by “What GitIM recorded.”

### Use page hierarchy to preserve focus

GitHub recommends descriptive titles, a one-sentence intro that confirms relevance, prerequisites immediately before procedures, and next steps that continue the most likely user journey. Further reading should be limited to links that help with the current task or topic. [GitHub article structure](https://docs.github.com/en/contributing/style-guide-and-content-model/contents-of-a-github-docs-article)

Cloudflare's information architecture follows a reader journey from overview and getting started through configuration, observability, reference, concepts, and platform information. Its how-to template requires a goal-oriented title, numbered steps, and an explicit result or next step. [Cloudflare information architecture](https://developers.cloudflare.com/style-guide/documentation-content-strategy/information-architecture/), [Cloudflare how-to content type](https://developers.cloudflare.com/style-guide/documentation-content-strategy/content-types/how-to/)

**GitIM principle:** page headings should describe reader questions or outcomes, while the sidebar groups content by journey:

- **Start:** understand the promise and complete one successful team interaction.
- **Use GitIM:** add agents, talk, organize work, run workflows, and review outcomes.
- **Understand GitIM:** messages as commits, repository as organization, routing, runtime, and distributed operation.
- **Reference:** protocol, files, CLI, HTTP API, states, limits, and operations.

## Reusable GitIM page anatomy

### 1. Outcome header

- Specific title framed as a capability or reader question.
- One-sentence answer stating value in the reader's language.
- Optional audience or prerequisite line.

### 2. Big-picture visual

- One diagram with three to five objects.
- Caption states the takeaway.
- A short paragraph explains why the relationship matters.

### 3. Worked example

- A realistic GitIM team, goal, and starting state.
- Three to six numbered moments.
- Visible result after each meaningful transition.
- A final product artifact: UI state, thread excerpt, card, run state, commit, or CLI output.

### 4. Concept expansion

Introduce concepts in the order the example encounters them. Each concept section contains:

- **What it is:** one plain-language definition.
- **Why it appears here:** its role in the example.
- **What it connects to:** one small relationship diagram or annotated product view.
- **What changes:** relevant state or lifecycle.
- **Explore further:** a disclosure or deep link for fields, variants, and edge cases.

Limit the first visible level to three or four major concepts. Related secondary concepts belong inside their parent section rather than in a flat grid.

### 5. Operational checkpoint

A compact “What GitIM records” panel connects product behavior to GitIM's differentiation:

- The file or object changed.
- The Git commit or history created.
- The identity responsible.
- The data boundary or repository ownership involved.

### 6. Reference and next step

- A compact table for exact fields, commands, states, or limits.
- One troubleshooting callout for the most likely failure.
- One primary next action and at most two related links.

## Page-specific visual opportunities

| Page | Primary visual | Worked example |
| --- | --- | --- |
| Quick start | Human → coordinator → specialist agents → Git repository | Create a workspace, add one agent, send one request, inspect its commit |
| Workspaces | Repository with human and agent clones | Compare local-directory and shared-remote workspace setup |
| Agents | Agent identity, provider session, clone, and runtime state | Add `planner`, start it, handle a message, stop it |
| Messaging | Message routing and parent-linked reply chain | Mention one agent and follow its auditable response |
| Work management | Project → card → assignee → discussion → archive | Turn a channel decision into a tracked card |
| Automation | Flow DAG paired with run-state progression | Run a release flow and observe node transitions |
| Quick sessions | Prompt → turns → result → saved reference | Explore a question without creating a durable team member |
| Protocol | Annotated `.thread` lines mapped to the UI conversation | Trace one reply from UI to file and Git commit |
| Runtime | WebUI → Runtime → daemon → provider → Git sync | Follow one message through the execution path |
| Distributed | Multiple local runtimes around one Git remote | Add a laptop or phone/WASM node without a service deployment |
| CLI and API | One task expressed as UI, CLI, and HTTP | Create and inspect the same card through two interfaces |
| Operations | Health signals feeding a recovery path | Diagnose an agent that stopped responding |

## Editorial acceptance criteria

A core documentation page is ready when:

- Its title and first sentence make the reader goal clear.
- The first viewport has one dominant idea.
- The page contains one meaningful example with a visible result.
- The primary diagram communicates one relationship and has accessible text.
- Major concepts appear in narrative order and secondary details expand from them.
- Reference facts are compact and separated from explanation.
- The reader can identify the next useful action without scanning a link wall.
