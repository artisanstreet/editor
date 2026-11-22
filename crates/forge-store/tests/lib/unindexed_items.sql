SELECT count(*) FROM items i WHERE NOT EXISTS (SELECT 1 FROM item_text t WHERE t.item_id = i.id)
