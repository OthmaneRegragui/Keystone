-- Login CAPTCHA (Cloudflare Turnstile), configured from the admin settings
-- screen rather than the environment, so keys can be rotated without a restart.
--
-- Defaults are empty/off: a fresh install behaves exactly as before.
-- `captcha_enabled` is an explicit admin toggle, and the captcha only becomes
-- active when it is on AND both keys are present.
INSERT INTO admin_settings (key, value, updated_at) VALUES
    ('captcha_enabled', 'false', NOW()),
    ('turnstile_site_key', '', NOW()),
    ('turnstile_secret_key', '', NOW())
ON CONFLICT (key) DO NOTHING;
