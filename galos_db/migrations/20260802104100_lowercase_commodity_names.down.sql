-- Only the constraint comes off. The spellings this migration folded
-- together were duplicate readings of one commodity, and which capital
-- letters each arrived under is not recorded anywhere, so a rollback keeps
-- the merged rows and lets mixed case back in.

ALTER TABLE commodities DROP CONSTRAINT commodities_name_is_lowercase;
