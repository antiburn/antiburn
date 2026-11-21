# Claude plugin directory submission

The plugin homepage is [antiburn.com/plugin](https://antiburn.com/plugin).
The submission repository is [antiburn/antiburn](https://github.com/antiburn/antiburn),
folder `plugins/antiburn`. Confirm the current tracked reference and review
state in the portal before changing either. The recommended
ongoing directory channel is a dedicated `claude-plugin-release` branch,
advanced only for tested plugin releases. A versioned tag is an alternative,
but each update requires selecting the new tag in the portal. See
[directory update channels](distribution.md#directory-update-channel) for both
procedures and the migration limits. The repository marketplace separately
selects an immutable artifact commit; its installation route stays on `main`.

See [directory submission guidance](distribution.md#claude-directory-submission)
for the permanent source and promotion requirements, and
[validation scope](validation.md) for supported checks and their limits.
A package version or tag does not establish directory acceptance or publication.
For an authorized tag-based submission, use `claude-plugin-v<version>` at the
validated artifact commit. Never move an existing versioned tag.

The owner validates and submits manually through the
[Claude developer portal](https://claude.ai/directory/manage), following the
[submission workflow](https://claude.com/docs/plugins/submit) and
[pre-submission checklist](https://claude.com/docs/plugins/pre-submission-checklist).
Automatic publication stays off. A GitHub Release is not required; a tag is
needed only when choosing the tag-based directory workflow.

Keep owner-specific submission records and captures outside maintained docs.
