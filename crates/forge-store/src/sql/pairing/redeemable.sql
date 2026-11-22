SELECT EXISTS(
  SELECT 1 FROM pair_codes
  WHERE code_sha256 = ?1 AND expires_at > ?2 AND redeemed_by IS NULL
)
