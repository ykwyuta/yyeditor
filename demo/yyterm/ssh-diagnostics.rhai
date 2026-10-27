// UTF-8 SSH macro. Edit the run identifier for each run.
// Run from an already connected SSH shell with Ctrl+Shift+F5.
const run_id = "20261023_001";
let marker = "YY_MACRO_DONE_" + run_id;
if wait_text(marker, 0) { throw "Change run_id: this marker is already on screen"; }

// Split the marker so input echo cannot satisfy wait_text.
send_line("pwd; uname -s; printf '%s%s\\n' YY_MACRO_DONE_ " + run_id);
wait_text(marker, 30);

let report = text_open("ssh-diagnostics.txt");
for line in screen_lines() { report.write_line(line); }
report.close();
log("Saved terminal screen to ssh-diagnostics.txt");
