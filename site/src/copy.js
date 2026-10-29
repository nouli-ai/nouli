// Copy buttons: a button's data-copy text goes to the clipboard, and the label confirms it.
export function copyButtons(root = document) {
  for (const button of root.querySelectorAll("button[data-copy]")) {
    const label = button.textContent;
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy);
        button.textContent = "copied";
        button.dataset.copied = "";
      } catch {
        button.textContent = "select it";
      }
      setTimeout(() => {
        button.textContent = label;
        delete button.dataset.copied;
      }, 1600);
    });
  }
}
