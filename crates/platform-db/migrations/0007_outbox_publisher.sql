ALTER TABLE outbox
    ADD COLUMN attempt_count integer NOT NULL DEFAULT 0,
    ADD COLUMN failure_count integer NOT NULL DEFAULT 0,
    ADD COLUMN next_attempt_at timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN claim_token uuid,
    ADD COLUMN claim_expires_at timestamptz,
    ADD COLUMN last_attempt_at timestamptz,
    ADD COLUMN last_failure_at timestamptz,
    ADD COLUMN last_error_code text,
    ADD COLUMN quarantined_at timestamptz,
    ADD CONSTRAINT outbox_attempt_count_nonnegative CHECK (attempt_count >= 0),
    ADD CONSTRAINT outbox_failure_count_nonnegative CHECK (failure_count >= 0),
    ADD CONSTRAINT outbox_claim_pair CHECK (
        (claim_token IS NULL) = (claim_expires_at IS NULL)
    ),
    ADD CONSTRAINT outbox_published_unclaimed CHECK (
        published_at IS NULL OR claim_token IS NULL
    ),
    ADD CONSTRAINT outbox_terminal_state CHECK (
        NOT (published_at IS NOT NULL AND quarantined_at IS NOT NULL)
    ),
    ADD CONSTRAINT outbox_quarantined_unclaimed CHECK (
        quarantined_at IS NULL OR claim_token IS NULL
    ),
    ADD CONSTRAINT outbox_error_code_shape CHECK (
        last_error_code IS NULL
        OR last_error_code ~ '^[a-z][a-z0-9._-]{0,63}$'
    );

DROP INDEX outbox_unpublished_idx;

CREATE INDEX outbox_unpublished_idx
    ON outbox (next_attempt_at, created_at, id)
    INCLUDE (claim_expires_at)
    WHERE published_at IS NULL AND quarantined_at IS NULL;

CREATE INDEX outbox_quarantined_idx
    ON outbox (quarantined_at, id)
    WHERE quarantined_at IS NOT NULL;
