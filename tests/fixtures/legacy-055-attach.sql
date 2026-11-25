-- The Postgres database beside tests/fixtures/legacy-055-attach, as the 0.5.5 release left it (pg_dump --inserts, 2026-11-24).
SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;
CREATE SCHEMA chronos;
SET default_tablespace = '';
SET default_table_access_method = heap;
CREATE TABLE chronos.merge_rows (
    merge_id text NOT NULL,
    n integer NOT NULL,
    table_name text NOT NULL,
    op text NOT NULL,
    key jsonb,
    before jsonb,
    after jsonb,
    at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE chronos.merges (
    id text NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL
);
CREATE TABLE public.t (
    id integer NOT NULL,
    v text NOT NULL
);
INSERT INTO chronos.merge_rows VALUES ('merge:909b2219c37e4b77e5b418d276e22c91', 0, 't', 'update', '{"id": "1"}', '{"v": "a", "id": "1"}', '{"v": "v055-a", "id": "1"}', '2026-11-24 16:06:11.160191+00');
INSERT INTO chronos.merge_rows VALUES ('merge:909b2219c37e4b77e5b418d276e22c91', 1, 't', 'insert', '{"id": "7"}', 'null', '{"v": "v055-a", "id": "7"}', '2026-11-24 16:06:11.160191+00');
INSERT INTO chronos.merge_rows VALUES ('merge:9ba4a72d3af540958380bc0e1c1067eb', 0, 't', 'delete', '{"id": "2"}', '{"v": "a", "id": "2"}', 'null', '2026-11-24 16:06:11.246923+00');
INSERT INTO chronos.merge_rows VALUES ('merge:9ba4a72d3af540958380bc0e1c1067eb', 1, 't', 'update', '{"id": "3"}', '{"v": "a", "id": "3"}', '{"v": "v055-b", "id": "3"}', '2026-11-24 16:06:11.246923+00');
INSERT INTO chronos.merges VALUES ('attach:7b1c0d06fb69ccd221a789f0b45a29e5', '2026-11-24 16:06:11.07881+00');
INSERT INTO chronos.merges VALUES ('merge:909b2219c37e4b77e5b418d276e22c91', '2026-11-24 16:06:11.160191+00');
INSERT INTO chronos.merges VALUES ('merge:9ba4a72d3af540958380bc0e1c1067eb', '2026-11-24 16:06:11.246923+00');
INSERT INTO public.t VALUES (4, 'a');
INSERT INTO public.t VALUES (5, 'a');
INSERT INTO public.t VALUES (6, 'a');
INSERT INTO public.t VALUES (1, 'v055-a');
INSERT INTO public.t VALUES (7, 'v055-a');
INSERT INTO public.t VALUES (3, 'v055-b');
ALTER TABLE ONLY chronos.merge_rows
    ADD CONSTRAINT merge_rows_pkey PRIMARY KEY (merge_id, n);
ALTER TABLE ONLY chronos.merges
    ADD CONSTRAINT merges_pkey PRIMARY KEY (id);
ALTER TABLE ONLY public.t
    ADD CONSTRAINT t_pkey PRIMARY KEY (id);
CREATE INDEX merge_rows_at ON chronos.merge_rows USING btree (at);
