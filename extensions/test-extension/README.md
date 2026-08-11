# Test Extension

This is a test extension that we use in the tests for the `extension` crate.

Originally based off the Gleam extension.

## Extension UI prototype

After installing this directory as a dev extension, add a temporary key binding
to open the prototype counter view:

```json
[
  {
    "bindings": {
      "ctrl-alt-u": [
        "extension::OpenView",
        {
          "extension_id": "test-extension",
          "view_id": "counter",
          "title": "Extension UI Counter"
        }
      ]
    }
  }
]
```
