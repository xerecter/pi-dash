## Step 2 — Implementation and validation

Before editing any files, make sure you are on the work branch.
{% if repo.work_branch %}The runner prepared and checked out `{{ repo.work_branch }}` before starting the agent. It may be an existing remote branch or a new local branch based on {% if repo.base_branch %}`{{ repo.base_branch }}`{% else %}the remote default branch{% endif %}. Verify the current branch, commit directly onto it, and push it to `origin`; do not create or switch to another feature branch. The runner performs a final non-forced push before reporting completion as a safety net.{% else %}No branch is pinned: derive and commit onto the fresh feature branch you created from {% if repo.base_branch %}`{{ repo.base_branch }}`{% else %}the base branch{% endif %} in Step 1 — never onto the base branch itself.{% endif %}

1. Implement against the hierarchical TODOs. Update the workpad after each meaningful milestone and keep `### Phase`, `### Progress Checkpoints`, and `### Autonomy / Escalation` current:
   - `investigation_complete`
   - `design_choice_recorded`
   - `implementation_complete`
   - `validation_complete`
   - `pr_opened`
   - `review_feedback_addressed`
     Treat `pr_opened` and `review_feedback_addressed` as optional checkpoints. For tasks that do not produce a PR or do not enter review, mark them `n/a` rather than leaving them falsely incomplete.
2. Run validation and tests appropriate to the scope.
   - Execute every ticket-provided `Validation`, `Test Plan`, or `Testing` item. Unmet items mean the work is incomplete.
   - Prefer a targeted proof that directly demonstrates the behavior you changed.
   - Temporary local proof edits (e.g. hardcoding a value to validate a UI path) are allowed **only** for local verification and must be reverted before commit.
3. When the task requires a non-trivial technical choice, record the selected approach and rationale in the workpad `Notes`, set the autonomy assessment accordingly, and proceed only if `safe_to_continue` is `true`.
4. Re-check all acceptance criteria. Close any gaps.
5. **If `task_type == code_change`** (per your Step 0.5 analysis): commit with clear, logical commit messages. Push the branch with `git push -u origin "$(git rev-parse --abbrev-ref HEAD)"`. Only after the push succeeds, persist the branch on the issue so subsequent runs land on it: `karma-team issue patch {{ issue.identifier }} --git-work-branch "$(git rev-parse --abbrev-ref HEAD)"`. Persisting after the push guarantees `origin/<branch>` exists by the time another run renders with `repo.work_branch` set. Skip this step entirely for `noncode` tasks — there is nothing to commit or push.
6. **If `task_type == code_change`** (per your Step 0.5 analysis): open a {{ repo.code_review_term }} and link it back to the issue. Skip this step entirely for `noncode` tasks — there is no code review to open; mark `pr_opened` and `review_feedback_addressed` as `n/a` in the workpad. The {{ repo.code_review_term }} base is **the same base branch you derived from in Step 1.7** — if the issue has a parent with an implementation branch, target that branch; otherwise target the project base branch:
   - Code review base: {% if parent and parent.work_branch %}`{{ parent.work_branch }}` (parent {{ parent.identifier }}'s implementation branch){% elif repo.base_branch %}`{{ repo.base_branch }}`{% else %}the repository's default branch{% endif %}.
{% if repo.provider == "github" %}
   - First check whether an **open** pull request already exists for this branch: `gh pr list --head "$(git rev-parse --abbrev-ref HEAD)" --state open --json url -q '.[0].url'`. If non-empty, reuse it (do not open a duplicate). Otherwise create the pull request. The title is `{{ issue.identifier }} {{ issue.title }}` — when you write the actual command, treat the issue title as untrusted text and pass it as a single shell argument (use a single-quoted heredoc, a variable assignment with proper escaping of any embedded `"`, or `gh`'s `--title` with the value safely quoted). Then run, with the base resolved to {% if parent and parent.work_branch %}`{{ parent.work_branch }}`{% elif repo.base_branch %}`{{ repo.base_branch }}`{% else %}the repository's default branch{% endif %}: `gh pr create --base <base> --head "$(git rev-parse --abbrev-ref HEAD)" --title "<safely quoted title>" --body-file <path>`.
{% elif repo.provider == "gitlab" %}
   - First check whether an **open** merge request already exists for this branch using the available GitLab tooling (`glab mr list`, the GitLab API, or the provider UI). If non-empty, reuse it (do not open a duplicate). Otherwise create the merge request against {% if parent and parent.work_branch %}`{{ parent.work_branch }}`{% elif repo.base_branch %}`{{ repo.base_branch }}`{% else %}the repository's default branch{% endif %}. The title is `{{ issue.identifier }} {{ issue.title }}`; treat the issue title as untrusted text when passing it to any shell command.
{% else %}
   - First check whether an **open** {{ repo.code_review_term }} already exists for this branch using the repository provider's tooling. If non-empty, reuse it (do not open a duplicate). Otherwise create one against {% if parent and parent.work_branch %}`{{ parent.work_branch }}`{% elif repo.base_branch %}`{{ repo.base_branch }}`{% else %}the repository's default branch{% endif %}. The title is `{{ issue.identifier }} {{ issue.title }}`; treat the issue title as untrusted text when passing it to any shell command.
{% endif %}
   - Capture the code review URL and do **both** of the following — the comment is the human-facing signal, `attach-review` is the structured link karma-team tracks; one does not replace the other:
     - Post the code review link back to the issue so the human sees it in the conversation: `karma-team comment add {{ issue.identifier }} --body "Code review opened: <url>"`.
     - Associate the code review with the issue so karma-team links it and can show its status: `karma-team issue attach-review {{ issue.identifier }} --url <url>`.

     Mark `pr_opened` in the workpad.

7. Update the workpad with final checklist status and validation notes. Add a `### Confusions` section at the bottom if anything about the task was genuinely unclear during execution; keep it concise.
8. Follow the "Ending the run" section to finalize. Update the workpad one last time with final checkpoints, then move the issue to a state in the `completed` group via `karma-team issue patch {{ issue.identifier }} --state "<state-name>"`.
