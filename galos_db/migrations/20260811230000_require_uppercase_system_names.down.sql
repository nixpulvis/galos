DROP INDEX systems_name;
CREATE INDEX systems_name ON systems USING btree (upper((name)::text));

ALTER TABLE markets DROP CONSTRAINT markets_system_name_uppercase;
ALTER TABLE systems DROP CONSTRAINT systems_name_uppercase;
