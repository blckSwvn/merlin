# TODO

### current

add UI component/widget system to allow for actual decent customizable UI like filepickers auto complete pop ups
    add function pointers to view for recalc, draw and extra behaviour

switch buffer not reset cursor to 0 0 rather remember per buffer
:splitv/splith replace the current leaf with a branch and move the leaf into the new branch instead of apending children
prevent viewclose on after :sh then :vc :vc it currently panics it should not be able to close the last view even if other branches are present


---

### future
copy paste, visual mode
autocompletion for cmd line like filepaths and commands
replacing arrays of structs(Aos) with struct of arrays(Soa)
add help cmd
handle resizing of term
vim motions
color highlighting

---

### maybe/unclear
marks
regex/fuzzy finding
sessions
registers
