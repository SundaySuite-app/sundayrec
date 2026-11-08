/**
 * En seksjon på Innstillinger: overskrift, én setning, og kortene.
 *
 * Overskriften er en `<h2>` med `tabIndex={-1}`: innholdslista øverst hopper
 * hit og flytter fokus, så en tastatur- eller skjermleserbruker lander der
 * synet gjør.
 */

import type { ComponentChildren } from "preact";

import { tDyn } from "../../i18n";
import type { SettingsSectionId } from "./sections";
import styles from "./setup.module.css";

export interface SettingsSectionProps {
  id: SettingsSectionId;
  /** Suffikset i `app.setup.sections.*` — se `sections.ts`. */
  sectionKey: string;
  /** Den korte setningen under overskriften. Utelates når barna har sin egen. */
  intro?: string;
  children: ComponentChildren;
}

export function SettingsSection({
  id,
  sectionKey,
  intro,
  children,
}: SettingsSectionProps) {
  return (
    <section
      id={id}
      aria-labelledby={`${id}-h`}
      data-testid={`setup-${id}`}
      class={styles.section}
    >
      <h2 id={`${id}-h`} tabIndex={-1} class={styles.sectionHead}>
        {tDyn("app.setup.sections", sectionKey)}
      </h2>
      {intro ? <p class={styles.sectionIntro}>{intro}</p> : null}
      {children}
    </section>
  );
}
