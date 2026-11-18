DELETE FROM pair_codes WHERE code_sha256 = ?1 AND expires_at > ?2
