SELECT pair_codes.redeemed_by, authorized_keys.id, authorized_keys.name,
  authorized_keys.public_key, authorized_keys.created_at, authorized_keys.last_seen
FROM pair_codes LEFT JOIN authorized_keys ON authorized_keys.id = pair_codes.redeemed_by
WHERE pair_codes.code_sha256 = ?1
