# karma-team MCP connector

The karma-team MCP connector exposes work items, projects, comments and workpads to
MCP-capable AI clients (e.g. Claude) as tools. It is a thin wrapper over the same
public REST surface (`/api/v1/`) that the `karma-team` CLI uses, so tool responses
mirror the REST serializers documented below.

## Issue web URL (`url`)

Every issue-returning tool includes an absolute, human-clickable `url` pointing
at the issue in the web UI, so an agent that has just filed or fetched work can
hand the user a link straight from the response.

Tools that carry the field:

| Tool                       | `url` points to                   |
| -------------------------- | --------------------------------- |
| `karma_team_get_issue`     | the issue                         |
| `karma_team_create_issue`  | the newly created issue           |
| `karma_team_update_issue`  | the issue                         |
| `karma_team_list_issues`   | each result row's issue           |
| `karma_team_search_issues` | each result row's issue           |
| `karma_team_comment_issue` | the issue the comment belongs to  |
| `karma_team_list_comments` | the issue each comment belongs to |

The same field flows through the `karma-team` CLI: `karma-team issue get <PROJ-123>`
prints the REST payload verbatim, so its JSON includes `url` as well.

### Shape

The value uses the canonical browse route, which the web UI resolves directly:

```
<web-base-url>/<workspace-slug>/browse/<PROJ>-<sequence_id>
```

for example `https://karma-team.example.com/eng/browse/ENG-42`.

### Where the base URL comes from

The base URL is taken from **deployment configuration** — the `WEB_URL` setting
(falling back to `APP_BASE_URL`) — never from the inbound request host. An MCP
server can sit behind a proxy or be reached over a non-public hostname, and
either would produce a link that does not work for the user.

### When it is omitted

If no web base URL is configured, the `url` field is **omitted entirely** rather
than emitted as a relative or otherwise broken link. A missing `url` is easy for
a client to handle; a broken one is not. Clients should treat `url` as optional.

There is no per-comment anchor in the UI, so comment tools link to the parent
issue rather than to the individual comment.
