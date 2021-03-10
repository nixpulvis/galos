CREATE TABLE system_faction_influences (
    system_address  bigint     NOT NULL REFERENCES systems,
    faction_id      integer    NOT NULL REFERENCES factions,
    new_influence   real       NOT NULL,
    old_influence   real       NOT NULL,
    new_timestamp   timestamp  NOT NULL,
    old_timestamp   timestamp  NOT NULL,

    FOREIGN KEY (system_address, faction_id)
    REFERENCES system_factions (system_address, faction_id)
);

CREATE FUNCTION insert_system_faction_influences()
RETURNS TRIGGER
AS
$$
BEGIN
    IF NEW.updated_at > OLD.updated_at AND
       NEW.influence != OLD.influence
    THEN
        INSERT INTO system_faction_influences (
            system_address,
            faction_id,
            new_influence,
            old_influence,
            new_timestamp,
            old_timestamp
        )
        VALUES(
            NEW.system_address,
            NEW.faction_id,
            NEW.influence,
            OLD.influence,
            NEW.updated_at,
            OLD.updated_at
        );
    END IF;

    RETURN NEW;
END
$$
LANGUAGE PLPGSQL;

CREATE TRIGGER system_faction_influence_changes
AFTER UPDATE
ON system_factions
FOR EACH ROW
EXECUTE PROCEDURE insert_system_faction_influences();
