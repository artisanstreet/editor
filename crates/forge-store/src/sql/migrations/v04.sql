UPDATE threads SET body = json_remove(
    json_set(body, '$.config.permissions',
        CASE WHEN json_extract(body, '$.config.approvals') = 'never'
               OR json_extract(body, '$.config.options.permission_mode') = 'bypassPermissions'
             THEN 'full' ELSE 'auto' END),
    '$.config.access', '$.config.approvals',
    '$.config.options.permission_mode', '$.config.options.mode');
UPDATE kv SET value = json_remove(
    json_set(value, '$.permissions',
        CASE WHEN json_extract(value, '$.approvals') = 'never'
               OR json_extract(value, '$.options.permission_mode') = 'bypassPermissions'
             THEN 'full' ELSE 'auto' END),
    '$.access', '$.approvals', '$.options.permission_mode', '$.options.mode')
WHERE key = 'default_engine_config' AND json_valid(value);
