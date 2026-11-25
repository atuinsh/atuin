# Reads a page in Chromium whose blanks between words are text nodes between comments, as on
# chatgpt.com. Chromium leaves those blanks out of the text that it reports, and capture puts
# back the ones that the page shows. Spans that touch stay one word.
capture_start 'apps = "all"' || return
chromium_ "file://$P/blanks.html"
sleep 8
can_focus && focus chromium
sleep 3
segment=$(find_segment --app chromium Touching spans)
text=$("$ATUIN" desktop-capture show "${segment:-00000000-0000-0000-0000-000000000000}" 2> /dev/null)
capture_stop
records
echo "$text"
echo
shows() {
    if grep -qxF "$2" <<< "$text"; then echo "ok    $1"; else echo "FAIL  $1"; fi
}
[ -n "$segment" ] && echo "ok    the page gets a segment" || echo "FAIL  the page gets a segment"
shows "blanks between words and beside a link are read" "Yes! Atuin is an excellent tool."
shows "words that wrap keep their blanks" "Words that wrap across several narrow lines."
shows "the column headings are read" "Feature | Atuin"
shows "a table cell keeps its blank" "Directory filtering | Yes"
shows "touching spans stay one word" \
    "Touching spans stay one word: 0b prefixes, a Bold first letter, and xy."
