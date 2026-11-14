-- Phase 14 (MySQL): granular guest permissions (WS2, ADR-0021) +
-- notification action center (WS5, ADR-0022) + guest invitations.
-- See the SQLite variants for the model narratives and migration mapping.

-- WS2: granular permissions (widened enum).
ALTER TABLE team_guest_permissions RENAME TO team_guest_permissions_old;
CREATE TABLE team_guest_permissions (
    id            CHAR(36) PRIMARY KEY,
    guest_id      CHAR(36) NOT NULL,
    permission    VARCHAR(32) NOT NULL,
    scope_type    VARCHAR(16) NOT NULL,
    scope_id      CHAR(36) NULL,
    metadata_json TEXT NULL,
    granted_by    CHAR(36) NULL,
    created_at    BIGINT NOT NULL,
    UNIQUE KEY uq_guest_permissions (guest_id, permission, scope_type, scope_id),
    CONSTRAINT fk_guest_permissions_guest FOREIGN KEY (guest_id) REFERENCES team_guests (id) ON DELETE CASCADE,
    CONSTRAINT chk_guest_permission CHECK (permission IN (
        'view_team', 'view_team_members', 'view_team_activity',
        'view_project_list', 'view_project', 'view_project_tasks',
        'view_project_timeline', 'view_project_board',
        'view_task', 'view_task_own', 'view_task_dates', 'view_task_activity',
        'view_task_comments', 'comment_task', 'view_task_attachments',
        'download_task_attachments', 'upload_task_attachments',
        'invite_guest', 'view_audit_log'
    )),
    CONSTRAINT chk_guest_scope CHECK (scope_type IN ('team', 'project', 'task', 'global'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-1'), prev.guest_id,
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
 SELECT CONCAT('gperm-', prev.id, '-tasks'), prev.guest_id, 'view_project_tasks', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
-- Legacy `view_project` covered every surface behind require_project_access
-- (detail, board, task list, timeline), so its granular expansion includes
-- board + timeline too — existing guests keep exactly the access they had.
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-board'), prev.guest_id, 'view_project_board', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-tl'), prev.guest_id, 'view_project_timeline', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_project';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-dates'), prev.guest_id, 'view_task_dates', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-comments'), prev.guest_id, 'view_task_comments', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
INSERT INTO team_guest_permissions
        (id, guest_id, permission, scope_type, scope_id, metadata_json, granted_by, created_at)
 SELECT CONCAT('gperm-', prev.id, '-activity'), prev.guest_id, 'view_task_activity', 'project', prev.scope_id, NULL, prev.granted_by, prev.created_at
   FROM team_guest_permissions_old prev
  WHERE prev.permission = 'view_task';
DROP TABLE team_guest_permissions_old;

-- WS2: guest invitations.
CREATE TABLE guest_invitations (
    id         CHAR(36) PRIMARY KEY,
    team_id    CHAR(36) NOT NULL,
    guest_id   CHAR(36) NOT NULL,
    invitee_email VARCHAR(255) NOT NULL,
    status     VARCHAR(16) NOT NULL DEFAULT 'pending',
    requested_by CHAR(36) NULL,
    decided_by CHAR(36) NULL,
    decided_at BIGINT NULL,
    created_at BIGINT NOT NULL,
    INDEX idx_guest_invitations_team (team_id, status),
    CONSTRAINT fk_guest_inv_team FOREIGN KEY (team_id) REFERENCES teams (id) ON DELETE CASCADE,
    CONSTRAINT fk_guest_inv_guest FOREIGN KEY (guest_id) REFERENCES team_guests (id) ON DELETE CASCADE,
    CONSTRAINT fk_guest_inv_requested FOREIGN KEY (requested_by) REFERENCES users (id) ON DELETE SET NULL,
    CONSTRAINT fk_guest_inv_decided FOREIGN KEY (decided_by) REFERENCES users (id) ON DELETE SET NULL,
    CONSTRAINT chk_guest_inv_status CHECK (status IN ('pending', 'approved', 'rejected'))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
