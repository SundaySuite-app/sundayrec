/**
 * Skilletegnet mellom to fakta på samme linje: «12. mars · 48 min · MP3».
 *
 * Én konstant og ikke fem kopier. Den sto som `const DOT = " · "` øverst i
 * RecordPage, LibraryPage, TrashPage, ExportPage og EditorPage — fem filer som
 * må si nøyaktig det samme for at listene skal se ut som én app, og fem steder
 * å glemme når noen bestemmer seg for en annen strek.
 *
 * ⚠️ Mellomrommene er en DEL av tegnet, ikke formatering rundt det. Kallerne
 * skriver `parts.join(DOT)`, altså er strengen selv det som skiller — en
 * `"·"` uten mellomrom ville limt fakta sammen på hver eneste rad.
 *
 * ⚠️ IKKE en i18n-nøkkel. En oversetter som får «·» i en strengfil har ingen
 * setning å oversette den i, og en gate som teller uoversatte bokstav-
 * sekvenser har rett i å la et rent skilletegn passere. Endres den, endres
 * den her.
 *
 * Opptaksoverlegget har med vilje SIN egen, uten mellomrom: der står tegnet
 * mellom to elementer som allerede har `gap` mellom seg, og mellomrommene
 * ville blitt doble. Se `app/pages/record/RecordingOverlay.tsx`.
 */
export const DOT = " · ";
