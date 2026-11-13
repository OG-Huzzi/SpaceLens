/**
 * Settings — the sixth destination, deliberately not one of the five pillars.
 *
 * SAFETY BOUNDARY: this screen changes preferences only. It has no scan,
 * cleanup, delete, or repair control, and it never calls the API. Every
 * state-changing action remains in Cleanup, behind preview + confirmation.
 */

import type { MessageKey } from "../i18n";
import { t } from "../i18n";
import { PageHeader, SectionHeading } from "../components/ui";
import { useSettings } from "../settings/SettingsProvider";
import {
  DENSITY_PREFERENCES,
  REDUCED_MOTION_PREFERENCES,
  SCREEN_IDS,
  SETTINGS_SCHEMA_VERSION,
  SETTINGS_STORAGE_KEY,
  SUPPORTED_LOCALES,
  THEME_PREFERENCES,
  isLocaleCode,
  type DensityPreference,
  type LocaleCode,
  type ReducedMotionPreference,
  type ScreenId,
  type ThemePreference
} from "../settings/schema";

const THEME_LABEL: Record<ThemePreference, MessageKey> = {
  system: "settings.theme.system",
  light: "settings.theme.light",
  dark: "settings.theme.dark"
};

const DENSITY_LABEL: Record<DensityPreference, MessageKey> = {
  comfortable: "settings.density.comfortable",
  compact: "settings.density.compact"
};

const MOTION_LABEL: Record<ReducedMotionPreference, MessageKey> = {
  system: "settings.motion.system",
  reduce: "settings.motion.reduce",
  allow: "settings.motion.allow"
};

const SCREEN_LABEL: Record<ScreenId, MessageKey> = {
  home: "app.nav.home",
  storage: "app.nav.storage",
  cleanup: "app.nav.cleanup",
  history: "app.nav.history",
  drives: "app.nav.drives"
};

const LOCALE_LABEL: Record<LocaleCode, MessageKey> = {
  en: "settings.language.en",
  de: "settings.language.de"
};

export function SettingsScreen() {
  const { settings, updateSettings, setScreenVisible, resetSettings, canPersist } =
    useSettings();
  const visibleCount = SCREEN_IDS.filter((id) => settings.visibleScreens[id]).length;

  return (
    <div className="screen screen--settings">
      <PageHeader
        eyebrow={t("settings.eyebrow")}
        title={t("settings.title")}
        description={t("settings.description")}
      />

      <section className="panel settings-panel" aria-labelledby="settings-language">
        <SectionHeading
          id="settings-language"
          title={t("settings.language.title")}
          description={t("settings.language.description")}
        />
        <div className="settings-row">
          <label className="settings-row__label" htmlFor="settings-locale">
            {t("settings.language.label")}
          </label>
          <select
            id="settings-locale"
            className="settings-select"
            value={settings.locale}
            onChange={(event) => {
              const value = event.target.value;
              if (isLocaleCode(value)) updateSettings({ locale: value });
            }}
          >
            {SUPPORTED_LOCALES.map((locale) => (
              <option key={locale} value={locale}>
                {t(LOCALE_LABEL[locale])}
              </option>
            ))}
          </select>
        </div>
      </section>

      <section className="panel settings-panel" aria-labelledby="settings-appearance">
        <SectionHeading
          id="settings-appearance"
          title={t("settings.appearance.title")}
          description={t("settings.appearance.description")}
        />

        <fieldset className="settings-fieldset">
          <legend>{t("settings.theme.legend")}</legend>
          <div className="settings-choices">
            {THEME_PREFERENCES.map((theme) => (
              <label className="settings-choice" key={theme}>
                <input
                  type="radio"
                  name="settings-theme"
                  value={theme}
                  checked={settings.theme === theme}
                  onChange={() => updateSettings({ theme })}
                />
                <span>{t(THEME_LABEL[theme])}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <fieldset className="settings-fieldset">
          <legend>{t("settings.density.legend")}</legend>
          <div className="settings-choices">
            {DENSITY_PREFERENCES.map((density) => (
              <label className="settings-choice" key={density}>
                <input
                  type="radio"
                  name="settings-density"
                  value={density}
                  checked={settings.density === density}
                  onChange={() => updateSettings({ density })}
                />
                <span>{t(DENSITY_LABEL[density])}</span>
              </label>
            ))}
          </div>
        </fieldset>

        <fieldset className="settings-fieldset">
          <legend>{t("settings.motion.legend")}</legend>
          <div className="settings-choices">
            {REDUCED_MOTION_PREFERENCES.map((reducedMotion) => (
              <label className="settings-choice" key={reducedMotion}>
                <input
                  type="radio"
                  name="settings-motion"
                  value={reducedMotion}
                  checked={settings.reducedMotion === reducedMotion}
                  onChange={() => updateSettings({ reducedMotion })}
                />
                <span>{t(MOTION_LABEL[reducedMotion])}</span>
              </label>
            ))}
          </div>
          <p className="settings-hint">{t("settings.motion.hint")}</p>
        </fieldset>
      </section>

      <section className="panel settings-panel" aria-labelledby="settings-screens">
        <SectionHeading
          id="settings-screens"
          title={t("settings.screens.title")}
          description={t("settings.screens.description")}
        />
        <p className="settings-note" id="settings-screens-note">
          {t("settings.screens.hiddenNote")}
        </p>
        <ul className="settings-screen-list">
          {SCREEN_IDS.map((id) => {
            const visible = settings.visibleScreens[id];
            const lastVisible = visible && visibleCount === 1;
            return (
              <li key={id}>
                <label className="settings-choice">
                  <input
                    type="checkbox"
                    checked={visible}
                    disabled={lastVisible}
                    aria-describedby={lastVisible ? "settings-screens-last" : undefined}
                    onChange={(event) => setScreenVisible(id, event.target.checked)}
                  />
                  <span>{t(SCREEN_LABEL[id])}</span>
                </label>
              </li>
            );
          })}
        </ul>
        <p className="settings-hint" id="settings-screens-last">
          {t("settings.screens.lastVisible")}
        </p>
      </section>

      <section className="panel settings-panel" aria-labelledby="settings-details">
        <SectionHeading id="settings-details" title={t("settings.details.title")} />
        <div className="settings-row">
          <div>
            <label className="settings-row__label" htmlFor="settings-advanced">
              {t("settings.details.showAdvanced")}
            </label>
            <p className="settings-hint">{t("settings.details.showAdvancedHint")}</p>
          </div>
          <input
            id="settings-advanced"
            className="settings-switch"
            type="checkbox"
            checked={settings.showAdvancedDetails}
            onChange={(event) =>
              updateSettings({ showAdvancedDetails: event.target.checked })
            }
          />
        </div>
      </section>

      <section className="panel settings-panel" aria-labelledby="settings-about">
        <SectionHeading
          id="settings-about"
          title={t("settings.about.title")}
          description={t("settings.about.description")}
        />
        <dl className="settings-facts">
          <div>
            <dt>{t("settings.about.storageKey")}</dt>
            <dd>
              <code>{SETTINGS_STORAGE_KEY}</code>
            </dd>
          </div>
          <div>
            <dt>{t("settings.about.schemaVersion")}</dt>
            <dd>{SETTINGS_SCHEMA_VERSION}</dd>
          </div>
          <div>
            <dt>{t("settings.about.locale")}</dt>
            <dd>
              <code>{settings.locale}</code>
            </dd>
          </div>
        </dl>

        {canPersist ? (
          <p className="settings-hint">{t("settings.about.saved")}</p>
        ) : (
          <p className="settings-warning" role="status">
            {t("settings.about.storageUnavailable")}
          </p>
        )}

        {/* Preferences only: this resets no files, scans, snapshots, or history. */}
        <div className="settings-reset">
          <button className="button button--quiet" type="button" onClick={resetSettings}>
            {t("settings.about.reset")}
          </button>
          <p className="microcopy">{t("settings.about.resetHint")}</p>
        </div>
      </section>
    </div>
  );
}
