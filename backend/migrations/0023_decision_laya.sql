-- What laya, Reflex's model lane, said for a decision (JSON `{"answer", "confidence"}`); NULL when it was not asked
-- (laya.rs). A lesson's decision carries it, so laya's threshold is fitted from the lessons.
ALTER TABLE decisions ADD COLUMN laya TEXT;
