/*
 * Progressive enhancement for the generator.
 *
 * Everything here is optional. Without it the form still posts, the server
 * still validates, and the suggestion chips still switch with the media type
 * (that part is CSS). What the script adds is the character counter, the
 * disabled state on the submit button, chips that fill the textarea, the
 * completion toast and copy-to-clipboard.
 *
 * Loaded as a module, so it is deferred and runs after the document is parsed.
 * No inline script anywhere: the CSP allows none.
 */

const MAKS_STANDARD = 4000;

/** Norwegian strings the script needs. Kept in one place, as in the Rust side. */
const TEKST = {
  kopiert: "Lenken er kopiert",
  ferdig: "Genereringen er ferdig",
  feilet: "Genereringen feilet",
};

/**
 * Keeps the character counter and the submit button in sync with the prompt.
 *
 * @param {HTMLFormElement} skjema
 */
function koblePromptTeller(skjema) {
  const felt = skjema.querySelector("#prompt");
  const teller = skjema.querySelector("#tegnteller");
  const knapp = skjema.querySelector("#generer");
  if (!felt || !teller || !knapp) return;

  const maks = Number(felt.dataset.maks) || MAKS_STANDARD;

  const oppdater = () => {
    // Count characters, not UTF-16 code units, so an emoji counts as one and
    // the number matches what the server enforces.
    const antall = [...felt.value].length;
    teller.textContent = `${antall}/${maks}`;

    const forLangt = antall > maks;
    teller.classList.toggle("text-red-400", forLangt);
    teller.classList.toggle("text-text-muted", !forLangt);
    felt.setAttribute("aria-invalid", forLangt ? "true" : "false");

    knapp.disabled = forLangt || felt.value.trim().length === 0;
  };

  felt.addEventListener("input", oppdater);
  oppdater();
}

/**
 * Makes a suggestion chip fill the prompt field.
 *
 * @param {HTMLFormElement} skjema
 */
function kobleForslag(skjema) {
  const felt = skjema.querySelector("#prompt");
  if (!felt) return;

  skjema.addEventListener("click", (hendelse) => {
    const chip = hendelse.target.closest(".forslag-chip");
    if (!chip) return;

    felt.value = chip.dataset.forslag ?? "";
    felt.dispatchEvent(new Event("input", { bubbles: true }));
    felt.focus();
  });
}

/**
 * Shows a short notice in the bottom right corner.
 *
 * @param {string} tekst
 */
function visVarsel(tekst) {
  const beholder = document.querySelector("#varsler");
  if (!beholder) return;

  const varsel = document.createElement("div");
  varsel.className =
    "pointer-events-auto rounded-card border border-border bg-surface px-4 py-3 text-sm text-text shadow-panel";
  varsel.textContent = tekst;
  beholder.append(varsel);

  // Respect a stated preference for less motion by skipping the fade.
  const roligBevegelse = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  window.setTimeout(() => {
    if (roligBevegelse) {
      varsel.remove();
      return;
    }
    varsel.style.transition = "opacity 240ms";
    varsel.style.opacity = "0";
    window.setTimeout(() => varsel.remove(), 260);
  }, 4000);
}

/** The open event stream, so a new job replaces the previous listener. */
let aapenStrom = null;

/**
 * Follows a job until it reaches a terminal status.
 *
 * The job lives in the database, not in this page, so this can be started
 * fresh on every load — including after a reload or a navigation back — and it
 * will pick up whatever is still running.
 *
 * @param {string} jobbId
 */
function folgJobb(jobbId) {
  aapenStrom?.close();

  const strom = new EventSource(`/api/jobs/${encodeURIComponent(jobbId)}/events`);
  aapenStrom = strom;

  strom.addEventListener("status", async (hendelse) => {
    let status;
    try {
      status = JSON.parse(hendelse.data).status;
    } catch {
      return;
    }
    if (status !== "succeeded" && status !== "failed" && status !== "cancelled") return;

    strom.close();
    if (aapenStrom === strom) aapenStrom = null;

    await hentKort(jobbId);
    visVarsel(status === "failed" ? TEKST.feilet : TEKST.ferdig);
  });

  strom.addEventListener("error", () => {
    // EventSource reconnects on its own. Falling back to one direct fetch
    // means a job that finished during a network blip is still picked up.
    if (strom.readyState === EventSource.CLOSED) {
      if (aapenStrom === strom) aapenStrom = null;
      void hentKort(jobbId);
    }
  });
}

/**
 * Re-fetches the card for a job and swaps it in.
 *
 * @param {string} jobbId
 */
async function hentKort(jobbId) {
  const beholder = document.querySelector("#resultat");
  if (!beholder) return;

  try {
    const svar = await fetch(`/api/jobs/${encodeURIComponent(jobbId)}/card`, {
      headers: { "hx-request": "true" },
    });
    if (!svar.ok) return;
    beholder.innerHTML = await svar.text();
    kobleResultat(beholder, { varsle: false });
  } catch {
    // Offline or navigating away; the card stays as it is and the next page
    // load will render the current state from the database.
  }
}

/**
 * Wires the buttons inside a job card, and starts following it if it is still
 * running.
 *
 * @param {ParentNode} rot
 * @param {{varsle?: boolean}} valg
 */
function kobleResultat(rot, valg = {}) {
  const kort = rot.querySelector("#jobbkort");
  if (!kort) return;

  const jobbId = kort.dataset.jobb;
  if (kort.dataset.venter === "true") {
    if (jobbId) folgJobb(jobbId);
    return;
  }

  // Move focus to the result so a screen reader and a keyboard user both land
  // on what just appeared.
  kort.focus({ preventScroll: true });
  kort.scrollIntoView({ behavior: "smooth", block: "nearest" });
  if (valg.varsle !== false) visVarsel(TEKST.ferdig);

  const kopier = kort.querySelector(".kopier-lenke");
  kopier?.addEventListener("click", async () => {
    const lenke = kopier.dataset.lenke;
    if (!lenke) return;
    try {
      await navigator.clipboard.writeText(new URL(lenke, window.location.href).href);
      visVarsel(TEKST.kopiert);
    } catch {
      // Clipboard access can be refused; there is nothing useful to do but
      // leave the user to copy the link themselves.
    }
  });

  const paaNytt = kort.querySelector(".generer-paa-nytt");
  paaNytt?.addEventListener("click", () => {
    const skjema = document.querySelector("#generator");
    const felt = skjema?.querySelector("#prompt");
    if (!skjema || !felt) return;

    felt.value = paaNytt.dataset.prompt ?? "";
    const medietype = paaNytt.dataset.medietype;
    const radio = skjema.querySelector(`input[name="media_type"][value="${medietype}"]`);
    if (radio) radio.checked = true;

    felt.dispatchEvent(new Event("input", { bubbles: true }));
    skjema.requestSubmit();
  });
}

function start() {
  const skjema = document.querySelector("#generator");
  if (skjema) {
    koblePromptTeller(skjema);
    kobleForslag(skjema);
  }

  // A job already running when the page loaded is rendered by the server;
  // pick it up so the page keeps following it across a reload.
  kobleResultat(document, { varsle: false });

  // HTMX replaces #resultat wholesale, so the new card is wired on each swap.
  document.body.addEventListener("htmx:afterSwap", (hendelse) => {
    if (hendelse.target?.id === "resultat") {
      kobleResultat(hendelse.target);
    }
  });
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", start, { once: true });
} else {
  start();
}
