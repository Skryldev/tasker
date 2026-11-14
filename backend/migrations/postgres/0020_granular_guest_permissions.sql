-- Phase 14 (PostgreSQL): notification action center (WS5, ADR-0022) +
-- granular guest permissions (WS2, ADR-0021) + guest invitations.
-- See the SQLite variant for the model narrative and migration mapping.

ALTER TABLE team_guest_permissions RENAME TO team_guest_permissions_old;
CREATE TABLE team_guest_permissions (
    id            TEXT PRIMARY KEY,
    guest_id      TEXT NOT NULL REFERENCES team_guests (id) ON DELETE CASCADE,
    permission    TEXT NOT NULL
        CHECK (permission IN (
            'view_team',
            'view_team_members',
            'view_team_activity',
            'view_project_list',
            'view_project',
            'view_project_tasks',
            'view_project_timeline',
            'view_project_board',
            'view_task',
            'view_task_own',
            'view_task_dates',
            'view_task_activity',
            'view_task_comments',
            'comment_task',
            'view_task_attachments',
            'download_task_attachments',
            'upload_task_attachments',
            'invite_guest',
            'view_audit_log'
        )),
    scope_type    TEXT NOT NULL CHECK (scope_type IN ('team', 'project', 'task', 'global')),
    scope_id      TEXT,
    metadata_json TEXT,
    granted_by    TEXT REFERENCES users (id) ON DELETE SET NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE (guest_id, permission, scope_type, scope_id)
);
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-1', prev.guest_id,
        CASE prev.permission
            WHEN 'view_team' THEN 'view_team'
            WHEN 'view_project' THEN 'view_project'
            WHEN 'view_task' THEN 'view_task'
            WHEN 'comment_task' THEN 'comment_task'
            WHEN 'view_activity' THEN 'view_task_activity'
        END,
        CASE WHEN prev.permission = 'view_team' THEN 'team' ELSE 'project' END,
        CASE WHEN prev.permission = 'view_team' THEN NULL ELSE prev.scope_id END,
        NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission IN ('view_team', 'view_project', 'view_task', 'comment_task', 'view_activity');
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-tasks', prev.guest_id, 'view_project_tasks', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
-- Legacy `view_project` covered every surface behind require_project_access
-- (detail, board, task list, timeline), so its granular expansion includes
-- board + timeline too — existing guests keep exactly the access they had.
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-board', prev.guest_id, 'view_project_board', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-tl', prev.guest_id, 'view_project_timeline', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-dates', prev.guest_id, 'view_task_dates', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-comments', prev.guest_id, 'view_task_comments', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT 'gperm-' || prev.id || '-activity', prev.guest_id, 'view_task_activity', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
DROP TABLE team_guest_permissions_old;

CREATE TABLE guest_invitations (
    id         TEXT PRIMARY KEY,
    team_id    TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    guest_id   TEXT NOT NULL REFERENCES team_guests (id) ON DELETE CASCADE,
    invitee_email TEXT NOT NULL,
    status     TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'rejected')),
    requested_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    decided_by TEXT REFERENCES users (id) ON DELETE SET NULL,
    decided_at BIGINT,
    created_at BIGINT NOT NULL
);
CREATE INDEX idx_guest_invitations_team ON guest_invitations (team_id, status);
