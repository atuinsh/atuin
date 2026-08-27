-- Project-local Neovim configuration for Atuin.
--
-- Opt out entirely:  vim.g.atuin_nvim = false
-- Or per-feature:    vim.g.atuin_nvim = { format_on_save = false }
--
-- Set either in your own init.lua; it loads before this file.

local user = vim.g.atuin_nvim
if user == false then return end

-- vim.fs.relpath and Client:supports_method() (colon-form) both require 0.11.
if vim.fn.has("nvim-0.11") == 0 then return end

local opts = vim.tbl_deep_extend("force", {
  colorcolumn = true,
  format_on_save = true,
}, type(user) == "table" and user or {})

-- ~2x a warm rust-analyzer format; generous enough for a cold server.
local FORMAT_TIMEOUT_MS = 2000

-- 'exrc' also searches parent directories, so cwd is not reliably the repo
-- root. Resolve our own location instead. See :h lua-script-location
local root = vim.fs.dirname(debug.getinfo(1, "S").source:gsub("^@", ""))

-- Scope every action to this repo. relpath() returns nil for paths outside
-- root, and unlike a string prefix match it rejects sibling dirs such as
-- "/tmp/atuin-evil" for a root of "/tmp/atuin".
local function in_project(buf)
  local name = vim.api.nvim_buf_get_name(buf)
  return name ~= "" and vim.fs.relpath(root, vim.fs.normalize(name)) ~= nil
end

local function is_project_rust_buf(buf)
  return vim.bo[buf].filetype == "rust" and in_project(buf)
end

local group = vim.api.nvim_create_augroup("atuin_nvim", { clear = true })

local function apply(buf)
  if not is_project_rust_buf(buf) or not opts.colorcolumn then
    return
  end

  -- "+1" is relative to 'textwidth', which .editorconfig sets from rustfmt's
  -- max_width -- so the ruler marks the first column rustfmt will not use, and
  -- the two cannot drift. Bail if 'textwidth' is unset (someone disabled
  -- editorconfig), or "+1" would resolve to column 1.
  if vim.bo[buf].textwidth == 0 then
    return
  end

  -- 'colorcolumn' is window-local, so set it on every window showing this buffer.
  for _, win in ipairs(vim.fn.win_findbuf(buf)) do
    vim.api.nvim_set_option_value("colorcolumn", "+1", { scope = "local", win = win })
  end
end

vim.api.nvim_create_autocmd("FileType", {
  group = group,
  pattern = "rust",
  callback = function(ev) apply(ev.buf) end,
})

-- FileType alone can fire before the buffer has a window (background loads),
-- which would skip the window-local colorcolumn. apply() is idempotent.
vim.api.nvim_create_autocmd("BufWinEnter", {
  group = group,
  pattern = "*.rs",
  callback = function(ev) apply(ev.buf) end,
})

-- Format on save via whichever Rust LSP the user already configured. This
-- never defines or starts a server -- if no Rust LSP is set up, LspAttach
-- simply never fires for these buffers and nothing happens.
vim.api.nvim_create_autocmd("LspAttach", {
  group = group,
  callback = function(ev)
    if not opts.format_on_save then return end
    if not is_project_rust_buf(ev.buf) then return end

    local client = vim.lsp.get_client_by_id(ev.data.client_id)
    if not client or not client:supports_method("textDocument/formatting") then return end
    -- Servers that support willSaveWaitUntil format on save themselves.
    if client:supports_method("textDocument/willSaveWaitUntil") then return end

    -- Register exactly one handler per buffer. LspAttach fires again on server
    -- restart and for every other client attaching to this buffer. Querying the
    -- autocmd list rather than a vim.b flag means the guard cannot outlive the
    -- handler it guards (augroup clear=true resets both).
    if #vim.api.nvim_get_autocmds({ group = group, event = "BufWritePre", buffer = ev.buf }) > 0 then
      return
    end

    vim.api.nvim_create_autocmd("BufWritePre", {
      group = group,
      buffer = ev.buf,
      callback = function()
        -- Resolve a live client at save time. Capturing client.id here would go
        -- stale the moment the server restarts.
        local c = vim.iter(vim.lsp.get_clients({ bufnr = ev.buf, method = "textDocument/formatting" }))
          :find(function(c) return not c:supports_method("textDocument/willSaveWaitUntil") end)
        if c then
          vim.lsp.buf.format({ bufnr = ev.buf, id = c.id, timeout_ms = FORMAT_TIMEOUT_MS })
        end
      end,
    })
  end,
})
