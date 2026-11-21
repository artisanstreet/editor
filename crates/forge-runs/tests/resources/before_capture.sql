UPDATE resource_capture SET from_position = (SELECT MAX(position) FROM items);
