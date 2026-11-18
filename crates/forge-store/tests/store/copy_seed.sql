INSERT INTO projects SELECT id,seq,body FROM seed.projects; INSERT INTO threads SELECT id,seq,body FROM seed.threads; DETACH DATABASE seed;
