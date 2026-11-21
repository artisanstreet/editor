SELECT DISTINCT j.value FROM "__TABLE__" t, json_tree(t.body) j
WHERE json_valid(t.body) AND j.type = 'text' AND length(j.value) = 64
