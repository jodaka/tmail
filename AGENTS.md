## Rust development

This project uses the standard Rust toolchain. Document code using comments.

### Required checks

After modifying Rust code:

- Run `cargo fmt --all`
- Run `cargo check --all-targets --all-features`
- Run `cargo clippy --all-targets --all-features -- -D warnings`
- Run `cargo test --all-features`

All checks must pass before considering a task complete.

Do not ignore compiler, rust-analyzer, Clippy, formatting, or test errors. Fix the underlying problem rather than suppressing warnings unless suppression is explicitly justified.

Do not git commit anything unless EXPLICITLY asked to do so. 
Do not push anything anywhere unless EXPLICITLY asked to do so. 

When in doubt about anything stop and ask human, do not guess, unless EXPLICITLY asked. 

<!-- BEGIN KATA (managed by `kata init --with-agents`) -->
## Kata is the system of record for intent.

- Never `kata delete` or `kata purge` without explicit user authorization.

~~~dot
digraph kata {
  rankdir=TB; node [shape=box];

  arrive   [shape=diamond label="Work arrives"];
  search   [label="Search first; reuse an open issue\nor create one"];
  route    [shape=diamond label="Work it, or delegate it?"];

  subgraph cluster_work {
    label="Working a kata-tracked issue";
    claim  [label="On claim or start, mark it actively tracked:\nkata meta set <ref> work.attention ok\nIn-flight work becomes visible to coordinators\nand dashboards from the moment it is grabbed."];
    branch [label="If the work happens on a dedicated branch, stamp it once:\nkata meta set <ref> work.branch <branch>\nor bind at creation:\nkata create ... --meta work.branch=<branch> --idempotency-key <key>"];
    live   [label="Keep your live state truthful on the issue:\nkata meta set <ref> work.attention stuck|needs-human|ok\nwith a one-line kata meta set <ref> work.attention_msg \"<why>\"\nRaise stuck when you cannot proceed, needs-human when you want\ninput or review (you may keep working), and clear back to ok\nwhen unblocked."];
    claim -> branch -> live;
  }

  subgraph cluster_delegate {
    label="Delegating work as separate issues (fan-out/join)";
    fanout [label="Create each delegated child with\n--parent <epic-or-coordinating-issue>,\n--meta work.branch=..., and an idempotency key;\ncapture refs from --json (.issue.short_id).\nAdd dependency links only for actual prerequisites."];
    join   [label="Join with kata wait <refs> --until attention --any\nMatches needs-human or stuck; a close also completes the wait,\nand the reported reason distinguishes which. Use --timeout so a\nwrapper can tell timeout from satisfaction."];
    coord  [label="As coordinator you read work.* —\nyou never write it on issues you delegated."];
    fanout -> join -> coord;
  }

  done     [shape=diamond label="Verified complete?"];
  close    [label="kata close <ref> --done\nwith a message and evidence"];
  review   [label="kata label add <ref> needs-review\nplus a comment on what remains"];
  park     [shape=diamond label="Park it?"];
  schedule [label="kata schedule <ref> <date-or-time>\nsets scheduled_on; clear with -"];
  someday  [label="kata meta set <ref> someday true --json-value\nclear with kata meta unset <ref> someday"];

  arrive -> search -> route;
  route -> claim   [label="work it"];
  route -> fanout  [label="delegate it"];
  route -> park    [label="record only"];
  live  -> done;
  coord -> done;
  done -> close    [label="yes"];
  done -> park     [label="no, stopping"];
  park -> schedule [label="start date known"];
  park -> someday  [label="no date"];
  park -> review   [label="no"];

  always [shape=note label="Always: one writer per key. work.* on closed issues is meaningless —\nnever write it there, ignore it when reading. Never end a session with\nthe signal stale: before stopping, either close the issue or set the\nattention pair to reflect the hand-off."];

  relationships [shape=note label="Relationships: Parent links express containment and roll-up only;\nthey do not gate readiness, and a parent cannot close with open children.\nUse --blocks <dependent> / --blocked-by <prerequisite>\nonly for real prerequisites; those links gate kata ready.\nUse --related <ref> for context only.\nkata wait observes state; it does not require a dependency edge."];

  gate [shape=note label="A future scheduled_on or someday=true keeps an issue\nout of ready and next. kata deadline <ref> <date-or-time>\nsets deadline_on, which never gates either."];
}
~~~
<!-- END KATA -->

Do not use OpenCode's internal todo list (`todowrite` / `todoread`).

For implementation work that would normally be broken into a todo list:

- Use Kata as the task tracking system.
- Create or reuse Kata issues for independently meaningful work items.
- Keep issue state updated while working.
- Close Kata issues only after verification.
- Do not create Kata issues for trivial implementation steps that are only useful within a single edit.

Use the Kata MCP tools when available rather than shelling out to the `kata` CLI.
