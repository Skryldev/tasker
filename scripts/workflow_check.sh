#!/usr/bin/env bash
# Phase 2 workflow + notification verification against a running server.
# Usage: scripts/workflow_check.sh <base-url>
set -u
BASE="${1:-http://localhost:8085}"
C=$(mktemp); M=$(mktemp); R=$(mktemp)
trap 'rm -f "$C" "$M" "$R"' EXIT

code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }
jqp() { python -c "import sys,json; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$2" 2>/dev/null; }

curl -s -c $C -X POST $BASE/api/v1/auth/login -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"super-secure-pass-1"}' -o /dev/null
curl -s -c $M -X POST $BASE/api/v1/auth/login -H 'Content-Type: application/json' \
  -d '{"email":"member@example.com","password":"member-pass-123"}' -o /dev/null

# Self-contained setup: fresh team, member, project on every run.
curl -s -b $C -X POST $BASE/api/v1/teams -H 'Content-Type: application/json'   -d "{\"name\":\"WF Team $(date +%s)\"}" > /dev/null
T=$(curl -s -b $C $BASE/api/v1/teams | python -c "import sys,json;items=json.load(sys.stdin)['items'];items.sort(key=lambda t:t['created_at']);print(items[-1]['id'])")
curl -s -b $C -X POST $BASE/api/v1/teams/$T/members -H 'Content-Type: application/json'   -d '{"email":"member@example.com","display_name":"Mona Member","role":"member","temp_password":"member-pass-123"}' > /dev/null
curl -s -c $M -X POST $BASE/api/v1/auth/login -H 'Content-Type: application/json'   -d '{"email":"member@example.com","password":"member-pass-123"}' -o /dev/null
P=$(curl -s -b $C -X POST $BASE/api/v1/teams/$T/projects -H 'Content-Type: application/json'   -d '{"name":"WF Project"}' | python -c "import sys,json;print(json.load(sys.stdin)['id'])")
MID=$(curl -s -b $C $BASE/api/v1/teams/$T/members | python -c "
import sys,json
for m in json.load(sys.stdin)['items']:
    if m['role']=='member': print(m['user']['id']); break")
RID=$(curl -s -b $C $BASE/api/v1/teams/$T/members | python -c "
import sys,json
for m in json.load(sys.stdin)['items']:
    if m['role']=='owner': print(m['user']['id']); break")

# member (not manager) tries to set admin as reporter -> 403
CODE=$(code -b $M -X POST $BASE/api/v1/projects/$P/tasks -H 'Content-Type: application/json' \
  -d "{\"title\":\"Reporter test\",\"reporter_id\":\"$RID\"}")
echo "member selects other reporter -> $CODE (expect 403)"

TASK=$(curl -s -b $M -X POST $BASE/api/v1/projects/$P/tasks -H 'Content-Type: application/json' \
  -d "{\"title\":\"P2 flow\",\"assignee_id\":\"$MID\"}" | python -c "import sys,json;print(json.load(sys.stdin)['id'])")
st() { curl -s -b $M $BASE/api/v1/tasks/$TASK | python -c "
import sys,json
t=json.load(sys.stdin)
print('status='+t['status'], 'res='+str(t.get('resolution')), 'note='+str(t.get('resolution_note')))"; }

echo "created:        $(st)"
curl -s -b $M -X PATCH $BASE/api/v1/tasks/$TASK -H 'Content-Type: application/json' \
  -d '{"status":"in_progress"}' -o /dev/null
echo "in_progress:    $(st)"

echo "submit:         $(code -b $M -X POST $BASE/api/v1/tasks/$TASK/submit) (expect 200)"
echo "after submit:   $(st)"
echo "member accepts: $(code -b $M -X POST $BASE/api/v1/tasks/$TASK/accept) (expect 403)"
echo "blank reject:   $(code -b $C -X POST $BASE/api/v1/tasks/$TASK/reject -H 'Content-Type: application/json' -d '{"reason":"  "}') (expect 422)"
echo "admin accepts:  $(code -b $C -X POST $BASE/api/v1/tasks/$TASK/accept) (expect 200)"
echo "after accept:   $(st)"
echo "stale accept:   $(code -b $C -X POST $BASE/api/v1/tasks/$TASK/accept) (expect 409)"

# reopen by admin, member works, submits again, admin rejects with reason
curl -s -b $C -X PATCH $BASE/api/v1/tasks/$TASK -H 'Content-Type: application/json' \
  -d '{"status":"in_progress"}' -o /dev/null
echo "reopen:         $(st) (resolution cleared)"
curl -s -b $M -X POST $BASE/api/v1/tasks/$TASK/submit -o /dev/null
echo "reject w/reason:$(code -b $C -X POST $BASE/api/v1/tasks/$TASK/reject -H 'Content-Type: application/json' -d '{"reason":"fix the intro"}') (expect 200)"
echo "after reject:   $(st)"

echo "-- notifications --"
echo "member:  $(curl -s -b $M $BASE/api/v1/notifications | python -c "
import sys,json
for n in json.load(sys.stdin)['items']: print(' ', n['event'], '|', n['summary'])")"
echo "admin:   $(curl -s -b $C $BASE/api/v1/notifications | python -c "
import sys,json
for n in json.load(sys.stdin)['items']: print(' ', n['event'], '|', n['summary'])")"
BEFORE=$(curl -s -b $M $BASE/api/v1/notifications/unread-count | python -c "import sys,json;print(json.load(sys.stdin)['count'])")
NID=$(curl -s -b $M "$BASE/api/v1/notifications?unread=true" | python -c "import sys,json;print(json.load(sys.stdin)['items'][0]['id'])")
echo "mark read:     $(code -b $M -X POST $BASE/api/v1/notifications/$NID/read) + idempotent $(code -b $M -X POST $BASE/api/v1/notifications/$NID/read)"
AFTER=$(curl -s -b $M $BASE/api/v1/notifications/unread-count | python -c "import sys,json;print(json.load(sys.stdin)['count'])")
echo "unread count:  before=$BEFORE after=$AFTER (expect after = before-1)"
echo "read-all:      $(code -b $M -X POST $BASE/api/v1/notifications/read-all) then count=$(curl -s -b $M $BASE/api/v1/notifications/unread-count | python -c "import sys,json;print(json.load(sys.stdin)['count'])")"
echo "foreign id:    $(code -b $M -X POST $BASE/api/v1/notifications/00000000-0000-0000-0000-000000000000/read) (expect 404)"
echo "unread filter: total=$(curl -s -b $M "$BASE/api/v1/notifications" | python -c "import sys,json;print(json.load(sys.stdin)['total'])") unread=$(curl -s -b $M "$BASE/api/v1/notifications?unread=true" | python -c "import sys,json;print(json.load(sys.stdin)['total'])")"
echo "mention:       $(code -b $C -X POST $BASE/api/v1/tasks/$TASK/comments -H 'Content-Type: application/json' -d '{"body":"hey @member@example.com look"}')"
echo "mention notif: $(curl -s -b $M $BASE/api/v1/notifications | python -c "
import sys,json
n=json.load(sys.stdin)['items'][0]; print(n['event'], '|', n['summary'])")"
echo "activity:      $(curl -s -b $C $BASE/api/v1/tasks/$TASK/activity | python -c "
import sys,json
print(' '.join(a['event'] for a in json.load(sys.stdin)['items']))")"
