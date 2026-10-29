# API (OpenAPI 3.1)

`openapi.yaml` is the **authoritative** API contract. `openapi.json` is the
machine-generated form (served by the backend at `/api/v1/openapi.json`) and
`../frontend/src/api/schema.d.ts` is the generated TypeScript type source.

## Regeneration flow

```bash
# after editing openapi.yaml:
cd api
node -e "
  const yaml = require('../frontend/node_modules/js-yaml');
  const fs = require('fs');
  fs.writeFileSync('openapi.json', JSON.stringify(yaml.load(fs.readFileSync('openapi.yaml','utf8')), null, 2));
"
cd ../frontend && npm run generate:api
```

## Conventions (enforced by the contract)

- Base path `/api/v1`; breaking changes would introduce `/api/v2`.
- Errors: RFC 9457 `application/problem+json` with an `errors[]` field-error
  extension for validation failures. Types:
  `https://tasker.dev/problems/<slug>`.
- Pagination: `page` (1-based), `page_size` (1–100, default 25) →
  `{items, page, page_size, total}`; server-side always.
- Filtering: comma-separated enums (`status=todo,in_progress`), `q`
  substring search, date range `due_before`/`due_after`, whitelisted `sort`
  and `order`.
- Auth: cookie `tasker_session` (see docs/security.md).

## Validation

CI runs `redocly lint` against `openapi.yaml`
(`.github/workflows/ci.yml`). Contract drift is limited by generating the
TypeScript types from the document and by asserting response shapes in the
smoke/E2E suites (representative success, validation, authentication, and
authorization responses are compared against the documented status codes and
problem types in `backend/tests/integration.rs`).
