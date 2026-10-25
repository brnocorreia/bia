-- $1: name
SELECT nonce, ciphertext FROM secrets WHERE name = $1
