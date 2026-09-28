-- Schema version 2 of a node's store: version 1 and this side table. It lists every recipient of
-- a stored transaction (the items of a transfer, 1/6), so the API's address criteria find a
-- transaction by any of its recipients, as the reference's containment query on the transfers of
-- the asset does. Each recipient is listed once per transaction, in the order the transaction
-- first names it; the rows go with their transaction.
CREATE TABLE transaction_recipients (
    transaction_id TEXT NOT NULL REFERENCES transactions (id),
    recipient_id   TEXT NOT NULL,
    UNIQUE (transaction_id, recipient_id)
) STRICT;
CREATE INDEX transaction_recipients_recipient_id ON transaction_recipients (recipient_id);
