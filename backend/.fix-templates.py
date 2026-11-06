import io

p = "src/workspace.rs"
lines = io.open(p, encoding="utf-8").read().split("\n")
out = []
i = 0
n = len(lines)
INSERT_OLD = "INSERT INTO task_templates (id, project_id, name, default_title, default_description, default_priority, created_by, created_at, updated_at)"
INSERT_NEW = "INSERT INTO task_templates (id, project_id, name, default_title, default_description, default_priority, default_subtasks, created_by, created_at, updated_at)"
UPDATE_OLD = "UPDATE task_templates SET name = ?, default_title = ?, default_description = ?, default_priority = ?, updated_at = ?"
UPDATE_NEW = "UPDATE task_templates SET name = ?, default_title = ?, default_description = ?, default_priority = ?, default_subtasks = ?, updated_at = ?"
inserted = 0
while i < n:
    l = lines[i]
    if INSERT_OLD in l:
        # find the args block above; add `.s(&subtasks_json)` after the last `.i(now)` binding
        j = i
        while "let mut args = Args::new();" not in lines[j]:
            j -= 1
        binds = [m for m in range(j, i) if ".i(now)" in lines[m]]
        pos = binds[-1] + 1 if binds else i
        lines.insert(pos, "        .s(&subtasks_json)")
        i += 1
        n = len(lines)
        l = lines[i]
        out.append(l.replace(INSERT_OLD, INSERT_NEW))
        i += 1
        inserted += 1
        continue
    if UPDATE_OLD in l:
        out.append(l.replace(UPDATE_OLD, UPDATE_NEW))
        # find the args block above; add `.s(&subtasks_json)` BEFORE the last `.i(now)` (order matters: SET order)
        j = i
        while "let mut args = Args::new();" not in lines[j]:
            j -= 1
        binds = [m for m in range(j, i) if ".i(now)" in lines[m]]
        pos = binds[-1]  # before `.i(now)`
        lines.insert(pos, "        .s(&subtasks_json)")
        i += 1
        n = len(lines)
        inserted += 1
        continue
    out.append(l)
    i += 1
io.open(p, "w", encoding="utf-8", newline="\n").write("\n".join(out))
print("patched", inserted)
