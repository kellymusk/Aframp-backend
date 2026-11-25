# Proposal: List active sessions and revoke other devices (#1340)

## Summary

Merchants currently have no way to see where they are signed in, or to sign out a
lost or compromised device. This proposal adds session tracking and two endpoints
that let an authenticated merchant list their active sessions and revoke a
specific one. It builds on the revocation list introduced in #1227.

## Goals

- Track active sessions per merchant with device, IP, `created_at`, and `last_seen`.
- `GET /sessions` — list the authenticated merchant's active sessions.
- `DELETE /sessions/{id}` — revoke a specific session.
- Revoked sessions stop working immediately.
- Document both endpoints in `openapi.yaml` (source of truth for the frontend).

## Non-goals

- Frontend UI (tracked in kellymusk/Aframp#992).
- Session management for non-merchant principals.

## Data model

A session record is created on successful authentication and stored alongside the
existing revocation list from #1227:

| Field        | Type      | Notes                                              |
| ------------ | --------- | -------------------------------------------------- |
| `id`         | string    | Opaque session identifier (also the token `jti`).  |
| `merchant_id`| string    | Owner of the session.                              |
| `device`     | string    | User-agent / device label captured at creation.    |
| `ip`         | string    | Client IP captured at creation.                    |
| `created_at` | timestamp | When the session was established.                  |
| `last_seen`  | timestamp | Updated on each authenticated request.             |
| `revoked_at` | timestamp | Null while active; set when revoked.               |

`last_seen` is refreshed on authenticated requests (throttled to avoid a write on
every call). Revocation sets `revoked_at` and adds the session `id` to the
revocation list from #1227 so the check is O(1) on the request path.

## Endpoints

### `GET /sessions`

Returns the authenticated merchant's active (non-revoked) sessions, most recently
seen first.

```json
{
  "sessions": [
    {
      "id": "sess_01H...",
      "device": "iPhone 15 / Safari",
      "ip": "203.0.113.7",
      "created_at": "2024-05-01T10:00:00Z",
      "last_seen": "2024-05-02T08:12:00Z",
      "current": true
    }
  ]
}
```

`current` marks the session making the request so the UI can label "this device".

### `DELETE /sessions/{id}`

Revokes the given session. Returns `204 No Content` on success. Returns `404` if
the session does not exist or does not belong to the authenticated merchant.
Revoking the current session is allowed and signs the caller out.

## Immediate revocation

Every authenticated request validates the session `id` against the revocation
list from #1227. Once `DELETE /sessions/{id}` records the revocation, subsequent
requests using that session fail with `401` immediately — no waiting for token
expiry.

## OpenAPI

Both endpoints, their schemas (`Session`, `SessionList`), and the `401`/`404`
responses are added to `openapi.yaml` in the same PR. The frontend consumes them
only through its `/backend` proxy and implements no logic of its own.

## Acceptance criteria

- [ ] Revoked sessions stop working immediately.
- [ ] Both endpoints documented in `openapi.yaml`.

## Related

#1227, #1308, kellymusk/Aframp#992
