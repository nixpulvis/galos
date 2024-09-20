CREATE FUNCTION systems_update_notify() RETURNS trigger AS $$
BEGIN
  IF TG_OP = 'INSERT' OR TG_OP = 'UPDATE' THEN
    PERFORM pg_notify('systems_update',
      json_build_object(
        'table', TG_TABLE_NAME,
        'row', NEW,
        'action', TG_OP)::text);
  ELSE
    PERFORM pg_notify('systems_update',
      json_build_object(
        'table', TG_TABLE_NAME,
        'row', OLD,
        'action', TG_OP)::text);
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER systems_notify_update AFTER UPDATE ON systems FOR EACH ROW EXECUTE PROCEDURE systems_update_notify();
CREATE TRIGGER systems_notify_insert AFTER INSERT ON systems FOR EACH ROW EXECUTE PROCEDURE systems_update_notify();
CREATE TRIGGER systems_notify_delete AFTER DELETE ON systems FOR EACH ROW EXECUTE PROCEDURE systems_update_notify();
