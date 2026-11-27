" Vim syntax file
" Language: morloc
" Maintainer: Zebulun Arendsee
" -----------------------------------------------------------------------------



" =============================================================================
"                             P R E A M B L E
" -----------------------------------------------------------------------------
if exists("b:current_syntax")
  finish
endif

let b:current_syntax = "loc"



" =============================================================================
"                             K E Y W O R D S
" -----------------------------------------------------------------------------

syn keyword reserved where module from as source export
syn keyword reserved do in let
syn keyword reserved newtype type record data object deriving
syn keyword reserved effect escapable
syn keyword reserved class instance import
syn keyword reserved infixl infixr infix

syn keyword s_constant True False Null Inf NaN -Inf

" -----------------------------------------------------------------------------
hi def link reserved Keyword
hi def link s_constant Constant



" =============================================================================
"                               S T R I N G S
" -----------------------------------------------------------------------------


" Interpolation regions inside strings
syn match s_escape_special contained /\\[nrtbfv0'"#\\]/
syn match s_escape_unicode contained /\\u[0-9a-fA-F]\{4}/
syn match s_escape_unicode contained /\\U[0-9a-fA-F]\{8}/
syn match s_escape_hex contained /\\x[0-9a-fA-F]\{1,2}/

" #{expression} is interpolation - contains everything except the string delimiters
syn region s_interpolation contained matchgroup=s_interp_delim start=/#{/ skip=/\\#{/ end=/}/
      \ contains=TOP

" Main string region - double-quoted strings
syn region s_string start=/"/ skip=/\\./ end=/"/
      \ contains=s_escape_special,s_escape_unicode,s_escape_hex,s_interpolation,s_interp_literal

syn match s_string /`[^`]\+`/

" Main string region - multiline strings double quotes
syn region s_string start=/"""/ skip=/\\./ end=/"""/
      \ contains=s_escape_special,s_escape_unicode,s_escape_hex,s_interpolation,s_interp_literal

" Main string region - multiline strings single quotes
syn region s_string start=/'''/ skip=/\\./ end=/'''/
      \ contains=s_escape_special,s_escape_unicode,s_escape_hex,s_interpolation,s_interp_literal

" Highlighting groups
hi def link s_string String
hi def link s_raw_string String
hi def link s_escape_special SpecialChar
hi def link s_escape_unicode SpecialChar
hi def link s_escape_hex SpecialChar
hi def link s_interp_literal Error
hi def link s_interp_delim Delimiter
hi def link s_interpolation Identifier

" =============================================================================
"                           G E T T E R  /  S E T T E R
" -----------------------------------------------------------------------------

" Match simple numeric index: .0, .1, .42, etc.
syntax match GetterIndex '\d\@<!\.\?\(\.[0-9]\+\|\.\a[a-zA-Z0-9_]*\)\+\.\?'
syntax match GetterIndex '\.(\@='
highlight link GetterIndex Special

" Optional: Match chained selectors as a unit: .x.y.z or .0.1.2
syntax match GetterChain '\<\(\.\(\d\+\|[a-zA-Z_][a-zA-Z0-9_]*\)\)\{2,}\>'
highlight link GetterChain Function


" =============================================================================
"                           P R I M A T I V E S
" -----------------------------------------------------------------------------

syn match s_num '\([a-zA-Z_.]\)\@<!\<[0-9]\+\>\([a-zA-Z_]\)\@!'
syn match s_hex '0[Xx][0-9a-fA-F]\+'
syn match s_oct '0[Oo][0-7]\+'
syn match s_bin '0[Bb][01]\+'
syn match s_dbl '\([a-zA-Z_.]\)\@<!\<[0-9]\+\.[0-9]\+\>\([a-zA-Z_]\)\@!'
syn match s_sci '\([a-zA-Z_.]\)\@<!\<[0-9]\+\.[0-9]\+[Ee]-\?[0-9]\+\>\([a-zA-Z_]\)\@!'

" -----------------------------------------------------------------------------
hi def link s_num      Number
hi def link s_hex      Number
hi def link s_oct      Number
hi def link s_bin      Number
hi def link s_dbl      Number
hi def link s_sci      Number

" =============================================================================
"                            O P E R A T O R S
" -----------------------------------------------------------------------------

" Generic operators (defined first so more specific patterns below take priority)
syn match operator /[!|@#$%^&*<>.:?/+=~-][!|@#$%^&*<>.:?/+=~-]*/
syn match operator /[()[\]{}]/

" -----------------------------------------------------------------------------
hi def link operator Operator

" =============================================================================
"                       P R A G M A S  &  I N T R I N S I C S
" -----------------------------------------------------------------------------

" %inline pragma (dim, like comments)
syn match s_pragma '%inline'

" Intrinsic functions: @show, @read, @hash, @save, @savem, @savej, @load, @lang
" An '@' glued to the end of an operand binds a name (`log@foo`, `xs.[0]@lbl`)
" and heads no intrinsic, so reject the characters that can end one.
syn match s_intrinsic '[A-Za-z0-9_`")\]}>]\@<!@[a-z][a-zA-Z0-9_]*'

" -----------------------------------------------------------------------------
hi def link s_pragma SpecialComment
hi def link s_intrinsic PreProc

" =============================================================================
"                          E F F E C T  T Y P E S
" -----------------------------------------------------------------------------

" Effect type annotations: <IO>, <IO, Error>, <Random>, etc.
syn match s_effect '<[A-Z][a-zA-Z0-9_]*\(,\s*[A-Z][a-zA-Z0-9_]*\)*>'

" -----------------------------------------------------------------------------
hi def link s_effect Type

" =============================================================================
"                          M I S C E L L A N I A
" -----------------------------------------------------------------------------
" syn match s_varlabel '[a-z]\w*@'

" -----------------------------------------------------------------------------
hi def link s_varlabel Special



" =============================================================================
"                             C O M M E N T S
" -----------------------------------------------------------------------------
" define todo highlighting
syn match s_todo /\(TODO\|NOTE\|FIXME\):/ contained
syn keyword s_todo XXX contained
syn match s_doc_tag /\(author\|desc\|email\|github\|bugs\|website\|maintainer\):/ contained
syn match s_doc_tag /@\(mime\|stdin\|with\|with.buffer\|render\|render.buffer\|arg\|short\|long\|name\|true\|false\|metavar\|parse\|group\|form\|example\|unroll\|many\|default\|form\|source\|list\|list.form\|list.source\|list.check\|list.check.path\|check.path\|stream\|epilogue\)/ contained

" define comments
syn match s_docstr '--\'.*' contains=s_doc_tag
syn match s_comment '--\'\@!.*' contains=s_todo

syn region s_comment start="{-" end="-}" contains=s_todo

" =============================================================================
"                               E R R O R S
" -----------------------------------------------------------------------------
syn match s_error '^#'

" -----------------------------------------------------------------------------
hi def link s_comment Comment
hi def link s_docstr  Comment
hi def link s_todo    Todo
hi def link s_doc_tag SpecialComment
hi def link s_tag     SpecialComment
hi def link s_error   Error
