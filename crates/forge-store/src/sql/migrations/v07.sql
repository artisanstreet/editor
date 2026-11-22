CREATE VIEW thread_search_content AS
 SELECT t.rowid AS rowid, COALESCE(json_extract(t.body,'$.title'),'') || ' ' || COALESCE(json_extract(p.body,'$.name'),'') AS content
 FROM threads t LEFT JOIN projects p ON p.id=json_extract(t.body,'$.project_id');
CREATE VIRTUAL TABLE threads_search USING fts5(content,content='thread_search_content',content_rowid='rowid',tokenize='trigram',columnsize=0);
INSERT INTO threads_search(threads_search) VALUES('rebuild');
CREATE TRIGGER thread_search_insert AFTER INSERT ON threads BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content WHERE rowid=NEW.rowid;
END;
CREATE TRIGGER thread_search_delete BEFORE DELETE ON threads BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content WHERE rowid=OLD.rowid;
END;
CREATE TRIGGER thread_search_before_update BEFORE UPDATE ON threads
 WHEN json_extract(OLD.body,'$.title') IS NOT json_extract(NEW.body,'$.title') OR json_extract(OLD.body,'$.project_id') IS NOT json_extract(NEW.body,'$.project_id') BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content WHERE rowid=OLD.rowid;
END;
CREATE TRIGGER thread_search_after_update AFTER UPDATE ON threads
 WHEN json_extract(OLD.body,'$.title') IS NOT json_extract(NEW.body,'$.title') OR json_extract(OLD.body,'$.project_id') IS NOT json_extract(NEW.body,'$.project_id') BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content WHERE rowid=NEW.rowid;
END;
CREATE TRIGGER project_search_before_update BEFORE UPDATE ON projects
 WHEN json_extract(OLD.body,'$.name') IS NOT json_extract(NEW.body,'$.name') BEGIN
 INSERT INTO threads_search(threads_search,rowid,content) SELECT 'delete',rowid,content FROM thread_search_content
 WHERE rowid IN (SELECT rowid FROM threads WHERE json_extract(body,'$.project_id')=OLD.id);
END;
CREATE TRIGGER project_search_after_update AFTER UPDATE ON projects
 WHEN json_extract(OLD.body,'$.name') IS NOT json_extract(NEW.body,'$.name') BEGIN
 INSERT INTO threads_search(rowid,content) SELECT rowid,content FROM thread_search_content
 WHERE rowid IN (SELECT rowid FROM threads WHERE json_extract(body,'$.project_id')=NEW.id);
END;
