t.rowid IN (SELECT rowid FROM item_search WHERE item_search MATCH ?1)
