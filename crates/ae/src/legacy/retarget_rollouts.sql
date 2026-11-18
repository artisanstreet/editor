UPDATE threads SET rollout_path = ?2 || substr(rollout_path, length(?1) + 1)
WHERE substr(rollout_path, 1, length(?1)) = ?1
