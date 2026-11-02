#!/usr/bin/env bash
# End-to-end API smoke test. Usage: ./smoke_api.sh <base_url> <admin_email> <admin_password>
# Exercises the core product workflow and prints one line per assertion.
set -u
BASE="$1"; EMAIL="$2"; PASS="$3"; COOKIES="$(mktemp)"; COOKIES2="$(mktemp)"; COOKIES3="$(mktemp)"
PASS_COUNT=0; FAIL_COUNT=0

check() { # $1 name, $2 expected-substr, $3 actual
  if echo "$3" | grep -q "$2"; then PASS_COUNT=$((PASS_COUNT+1)); echo "PASS: $1";
  else FAIL_COUNT=$((FAIL_COUNT+1)); echo "FAIL: $1 — expected [$2] got: $(echo "$3" | head -c 300)"; fi
}
jqget() { echo "$1" | jq -r "$2"; }

# --- login ---
R=$(curl -s -c "$COOKIES" -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' -d "{\"email\":\"$EMAIL\",\"password\":\"$PASS\"}")
check "login returns user" '"display_name"' "$R"
ADMIN_ID=$(jqget "$R" '.id')

# --- wrong password rejected ---
R=$(curl -s -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' -d "{\"email\":\"$EMAIL\",\"password\":\"wrong-password\"}")
check "wrong password -> 401" 'unauthenticated' "$R"

# --- create team ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams" -H 'Content-Type: application/json' -d '{"name":"Acme","description":"Smoke team"}')
check "create team" '"my_role":"owner"' "$R"
TEAM=$(jqget "$R" '.id')

# --- add member (provisioned with temp password) ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams/$TEAM/members" -H 'Content-Type: application/json' -d '{"email":"member@example.com","display_name":"Member","role":"member","temp_password":"member-pass-123"}')
check "add member" '"role":"member"' "$R"
MEMBER_ID=$(jqget "$R" '.user.id')
R=$(curl -s -c "$COOKIES2" -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' -d '{"email":"member@example.com","password":"member-pass-123"}')
check "member can login with temp password" '"display_name":"Member"' "$R"

# --- member cannot change own role (403) ---
R=$(curl -s -b "$COOKIES2" -X PATCH "$BASE/api/v1/teams/$TEAM/members/$MEMBER_ID" -H 'Content-Type: application/json' -d '{"role":"owner"}')
check "member cannot self-escalate role" 'forbidden' "$R"

# --- member cannot update team settings (403) ---
R=$(curl -s -b "$COOKIES2" -X PATCH "$BASE/api/v1/teams/$TEAM" -H 'Content-Type: application/json' -d '{"name":"Hacked"}')
check "member cannot update team" 'forbidden' "$R"

# --- cross-team isolation: a genuine outsider (user in another team) gets 404 ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams" -H 'Content-Type: application/json' -d '{"name":"Other"}')
TEAM2=$(jqget "$R" '.id')
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams/$TEAM2/members" -H 'Content-Type: application/json' -d '{"email":"outsider@example.com","display_name":"Outsider","role":"member","temp_password":"outsider-pass-1"}')
R=$(curl -s -c "$COOKIES3" -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' -d '{"email":"outsider@example.com","password":"outsider-pass-1"}')
check "outsider can login" '"display_name":"Outsider"' "$R"
R=$(curl -s -b "$COOKIES3" "$BASE/api/v1/teams/$TEAM/members")
check "outsider sees no team data" 'not-found' "$R"

# --- label ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams/$TEAM/labels" -H 'Content-Type: application/json' -d '{"name":"bug","color":"#ef4444"}')
check "create label" '"name":"bug"' "$R"
LABEL=$(jqget "$R" '.id')

# --- project ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/teams/$TEAM/projects" -H 'Content-Type: application/json' -d '{"name":"Website","description":"Launch site"}')
check "create project" '"status":"active"' "$R"
PROJECT=$(jqget "$R" '.id')

# --- member creates task (no assignee: the creator is the default reporter,
#     and one person must not be both assignee and reporter) ---
R=$(curl -s -b "$COOKIES2" -X POST "$BASE/api/v1/projects/$PROJECT/tasks" -H 'Content-Type: application/json' -d "{\"title\":\"Fix hero\",\"priority\":\"high\",\"label_ids\":[\"$LABEL\"]}")
check "member creates task" '"title":"Fix hero"' "$R"
TASK=$(jqget "$R" '.id')

# --- self-review creation is rejected (identity invariant) ---
R=$(curl -s -b "$COOKIES2" -X POST "$BASE/api/v1/projects/$PROJECT/tasks" -H 'Content-Type: application/json' -d "{\"title\":\"Self review\",\"assignee_id\":\"$MEMBER_ID\"}")
check "assignee+reporter overlap rejected" 'validation-failed' "$R"

R=$(curl -s -b "$COOKIES3" "$BASE/api/v1/tasks/$TASK")
check "outsider sees no task" 'not-found' "$R"

# --- member cannot assign someone else ---
R=$(curl -s -b "$COOKIES2" -X PATCH "$BASE/api/v1/tasks/$TASK" -H 'Content-Type: application/json' -d "{\"assignee_id\":\"$ADMIN_ID\"}")
check "member cannot assign others" 'forbidden' "$R"

# --- admin takes over as reporter, then assigns the member ---
R=$(curl -s -b "$COOKIES" -X PATCH "$BASE/api/v1/tasks/$TASK" -H 'Content-Type: application/json' -d "{\"reporter_id\":\"$ADMIN_ID\"}")
check "admin changes reporter" "$ADMIN_ID" "$(echo "$R" | jq -c .reporter)"
R=$(curl -s -b "$COOKIES" -X PATCH "$BASE/api/v1/tasks/$TASK" -H 'Content-Type: application/json' -d "{\"assignee_id\":\"$MEMBER_ID\"}")
check "admin assigns member" "$MEMBER_ID" "$(echo "$R" | jq -c .assignee)"

# --- generic PATCH no longer mutates status (400) ---
R=$(curl -s -b "$COOKIES" -X PATCH "$BASE/api/v1/tasks/$TASK" -H 'Content-Type: application/json' -d '{"status":"in_progress"}')
check "PATCH status rejected (400)" 'bad-request' "$R"

# --- assignee drives the lifecycle through the transition endpoint ---
R=$(curl -s -b "$COOKIES2" -X POST "$BASE/api/v1/tasks/$TASK/transition" -H 'Content-Type: application/json' -d '{"to":"todo"}')
check "assignee picks up (backlog->todo)" '"status":"todo"' "$R"
R=$(curl -s -b "$COOKIES2" -X POST "$BASE/api/v1/tasks/$TASK/transition" -H 'Content-Type: application/json' -d '{"to":"in_progress"}')
check "assignee starts work (todo->in_progress)" '"status":"in_progress"' "$R"

# --- subtask ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/tasks/$TASK/subtasks" -H 'Content-Type: application/json' -d '{"title":"Pick image"}')
check "create subtask" '"parent_task_id"' "$R"
SUBTASK=$(jqget "$R" '.id')
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/tasks/$SUBTASK/subtasks" -H 'Content-Type: application/json' -d '{"title":"Nested"}')
check "nested subtask rejected" 'one nesting level' "$R"

# --- comment ---
R=$(curl -s -b "$COOKIES2" -X POST "$BASE/api/v1/tasks/$TASK/comments" -H 'Content-Type: application/json' -d '{"body":"Working on it"}')
check "member comments" '"body":"Working on it"' "$R"
COMMENT=$(jqget "$R" '.id')

# --- admin cannot edit member's comment (policy: only author or Owner/Admin-edit) — admin CAN edit any ---
R=$(curl -s -b "$COOKIES" -X PATCH "$BASE/api/v1/comments/$COMMENT" -H 'Content-Type: application/json' -d '{"body":"Working on it (edited by admin)"}')
check "admin edits any comment" 'edited by admin' "$R"

# --- member tries to delete admin's comment ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/tasks/$TASK/comments" -H 'Content-Type: application/json' -d '{"body":"Admin note"}')
ADMIN_COMMENT=$(jqget "$R" '.id')
R=$(curl -s -b "$COOKIES2" -X DELETE "$BASE/api/v1/comments/$ADMIN_COMMENT")
check "member cannot delete others' comment" 'forbidden' "$R"

# --- board ---
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/projects/$PROJECT/board")
check "board has columns" '"columns"' "$R"
check "board shows in_progress task" 'in_progress' "$R"

# --- list + filters + pagination ---
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/projects/$PROJECT/tasks?status=in_progress&priority=high&page=1&page_size=10")
check "list filter status+priority" '"total":1' "$R"
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/projects/$PROJECT/tasks?q=hero")
check "list search q=" '"total":1' "$R"
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/projects/$PROJECT/tasks?sort=priority&order=asc")
check "list sort" '"items"' "$R"

# --- my tasks ---
R=$(curl -s -b "$COOKIES2" "$BASE/api/v1/me/tasks")
check "my tasks for member" '"project_name":"Website"' "$R"

# --- validation error shape ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/projects/$PROJECT/tasks" -H 'Content-Type: application/json' -d '{"title":"","priority":"bogus"}')
check "validation problem+json" 'validation-failed' "$R"
check "validation has errors array" '"errors"' "$R"

# --- activity history ---
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/tasks/$TASK/activity")
check "activity records status change" 'task_status_changed' "$R"
check "activity records assignee change" 'task_assignee_changed' "$R"

# --- unauthenticated access rejected ---
R=$(curl -s "$BASE/api/v1/teams")
check "unauthenticated teams -> 401" 'unauthenticated' "$R"

# --- logout revokes session ---
R=$(curl -s -b "$COOKIES" -X POST "$BASE/api/v1/auth/logout")
R=$(curl -s -b "$COOKIES" "$BASE/api/v1/me")
check "session revoked after logout" 'unauthenticated' "$R"

echo
echo "RESULT: $PASS_COUNT passed, $FAIL_COUNT failed"
rm -f "$COOKIES" "$COOKIES2" "$COOKIES3"
[ "$FAIL_COUNT" -eq 0 ]
