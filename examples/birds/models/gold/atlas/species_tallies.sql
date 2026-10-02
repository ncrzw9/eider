WITH tallies AS (
    SELECT hk, region, birds_seen: sum(bird_count)
    FROM silver.birds__sat_sighting__fieldbook
    GROUP BY ALL
)

SELECT species: COALESCE(tx.accepted_code, h.business_key), s.common_name, t.region, t.birds_seen
FROM silver.birds__hub_species AS h
JOIN silver.birds__sat_species__fieldbook AS s USING (hk)
JOIN tallies AS t USING (hk)
LEFT JOIN reference.birds__taxonomy AS tx ON tx.species_code = h.business_key;
