use core::panic;

use anyhow::Result;
use regex::Regex;
use swc_common::{
    self,
    sync::Lrc,
    FileName, SourceMap,
};
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax};
use swc_ecma_parser::{
    token::{IdentLike, Token, Word},
    Capturing, TsSyntax,
};
use types::{CodeLanguage, StringParser};

pub struct CodeParser {
    // language: CodeLanguage,
}

impl CodeParser {
    pub fn from_language(language: CodeLanguage) -> Self {
        match language {
            CodeLanguage::TSX => CodeParser {
                // language: CodeLanguage::TSX,
            },
            _ => panic!("Unsupported language"),
        }
    }

    pub fn tokenize_and_stem(&self, input: &str) -> Result<Vec<(String, Vec<String>)>> {
        // This code is obscure to me
        // I copied and pasted it from the swc project
        // Link: https://github.com/swc-project/swc/blob/499c8034133417dd01e864c1e11844f6ce9215dc/crates/swc_ecma_parser/examples/lexer.rs

        let cm: Lrc<SourceMap> = Default::default();
        // let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

        let fm: Lrc<swc_common::SourceFile> = cm.new_source_file(
            Lrc::new(FileName::Custom("test.tsx".into())),
            input.to_string(),
        );
        let lexer = Lexer::new(
            Syntax::Typescript(TsSyntax {
                tsx: true,
                ..Default::default()
            }),
            Default::default(),
            StringInput::from(&*fm),
            // TODO: Add comment to tokens
            None,
        );
        let capturing = Capturing::new(lexer);
        let mut parser = Parser::new_from(capturing);

        // We ignore errors, so we can parse as much as possible
        // TODO: should we collect this information and return it to the user?
        /*
        for e in parser.take_errors() {
            e.into_diagnostic(&handler).emit();
        }
        */

        let _module = parser.parse_module();
        // We ignore errors, so we can parse as much as possible
        // TODO: should we collect this information and return it to the user?
        //   .map_err(|e| e.into_diagnostic(&handler).emit())
        //   .expect("Failed to parse module.");

        let tokens: Vec<_> = parser.input().take();

        let tokens: Vec<_> = tokens
            .into_iter()
            .filter(|token| match &token.token {
                Token::Str { raw, .. } => raw.to_ascii_lowercase() != "'use client'",
                Token::JSXName { .. } => true,
                Token::JSXText { .. } => true,
                Token::Word(Word::Ident(IdentLike::Other(_))) => true,
                _ => false,
            })
            .filter_map(|d| {
                let t = match d.token {
                    Token::Str { raw, .. } => raw.to_string(),
                    Token::JSXName { name } => name.to_string(),
                    Token::JSXText { raw, .. } => raw.to_string(),
                    Token::Word(Word::Ident(IdentLike::Other(a))) => a.to_string(),
                    _ => return None,
                };

                if t.trim().is_empty() {
                    return None;
                }

                let token = t.trim().to_string().replace("'", "").replace("\"", "");
                let lower_case_token = token.to_lowercase();

                if lower_case_token.len() > 20 {
                    // We ignore long tokens
                    return None
                }

                let stemmed = calculate_stemmed(&token)
                    .into_iter()
                    .filter(|s| !s.is_empty() && *s != lower_case_token)
                    .collect();

                Some((lower_case_token, stemmed))
            })
            .collect();

        Ok(tokens)
    }
}

impl StringParser for CodeParser {
    fn tokenize_str_and_stem(&self, input: &str) -> Result<Vec<(String, Vec<String>)>> {
        self.tokenize_and_stem(input)
    }
}

fn calculate_stemmed(input: &str) -> Vec<String> {
    let re = Regex::new(r"[A-Z][a-z]*|[a-z]+").unwrap();
    re.find_iter(input)
        .map(|m| m.as_str().to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_calculate_stemmed() {
        let output = calculate_stemmed("QueryClientProvider");
        assert_eq!(output, vec!["query", "client", "provider"]);

        let output = calculate_stemmed("getQueryClient");
        assert_eq!(output, vec!["get", "query", "client"]);

        let output = calculate_stemmed("make_query_client");
        assert_eq!(output, vec!["make", "query", "client"]);

        let output = calculate_stemmed("pippo");
        assert_eq!(output, vec!["pippo"]);
    }

    #[test]
    fn test_simple() {
        let code = r#"
// In Next.js, this file would be called: app/layout.jsx
import Providers from './providers'

export default function RootLayout({ children }) {
  return (
    <html lang="en">
      <head />
      <body>
        <Providers>{children}</Providers>
      </body>
    </html>
  )
}"#
        .to_string();

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let t = parser.tokenize_and_stem(&code).unwrap();

        assert_eq!(
            t,
            vec![
                ("providers".to_string(), vec![]),
                ("./providers".to_string(), vec!["providers".to_string()]),
                (
                    "rootlayout".to_string(),
                    vec!["root".to_string(), "layout".to_string()]
                ),
                ("children".to_string(), vec![]),
                ("html".to_string(), vec![]),
                ("lang".to_string(), vec![]),
                ("en".to_string(), vec![]),
                ("head".to_string(), vec![]),
                ("body".to_string(), vec![]),
                ("providers".to_string(), vec![]),
                ("children".to_string(), vec![]),
                ("providers".to_string(), vec![]),
                ("body".to_string(), vec![]),
                ("html".to_string(), vec![])
            ]
        );
    }

    #[test]
    fn test_tokenize_and_stem() {
        let code = r#"
    // In Next.js, this file would be called: app/providers.tsx
    'use client'

    // Since QueryClientProvider relies on useContext under the hood, we have to put 'use client' on top
    import {
      isServer,
      QueryClient,
      QueryClientProvider,
    } from '@tanstack/react-query'

    function makeQueryClient() {
      return new QueryClient({
        defaultOptions: {
          queries: {
            // With SSR, we usually want to set some default staleTime
            // above 0 to avoid refetching immediately on the client
            staleTime: 60 * 1000,
          },
        },
      })
    }

    let browserQueryClient: QueryClient | undefined = undefined

    function getQueryClient() {
      if (isServer) {
        // Server: always make a new query client
        return makeQueryClient()
      } else {
        // Browser: make a new query client if we don't already have one
        // This is very important, so we don't re-make a new client if React
        // suspends during the initial render. This may not be needed if we
        // have a suspense boundary BELOW the creation of the query client
        if (!browserQueryClient) browserQueryClient = makeQueryClient()
        return browserQueryClient
      }
    }

    export default function Providers({ children }) {
      // NOTE: Avoid useState when initializing the query client if you don't
      //       have a suspense boundary between this and the code that may
      //       suspend because React will throw away the client on the initial
      //       render if it suspends and there is no boundary
      const queryClient = getQueryClient()

      return (
        <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
      )
    }"#.to_string();

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let t = parser.tokenize_and_stem(&code).unwrap();

        assert_ne!(t, vec![]);
    }

    #[test]
    fn test_tokenize_and_stem_th() {
        let code = r#"<th
  key={header.id}
  colSpan={header.colSpan}
  style={{ width: `${header.getSize()}px` }}
>"#;

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let t = parser.tokenize_and_stem(code).unwrap();

        assert_eq!(
            t,
            vec![("th".to_string(), vec![]), ("key".to_string(), vec![])]
        );
    }

    #[test]
    fn test_1() {
        // This code is not parsable from swc.
        // The parser stops and returns only "initialState"
        let code = r###"initialState?: Partial<
  VisibilityTableState &
  ColumnOrderTableState &
  ColumnPinningTableState &
  FiltersTableState &
  SortingTableState &
  ExpandedTableState &
  GroupingTableState &
  ColumnSizingTableState &
  PaginationTableState &
  RowSelectionTableState
>"###;

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let t = parser.tokenize_and_stem(code).unwrap();

        assert_eq!(
            t,
            vec![(
                "initialstate".to_string(),
                vec!["initial".to_string(), "state".to_string()]
            )]
        );
    }

    #[test]
    fn test_2() {
        let code = r###"import React from 'react'
import ReactDOM from 'react-dom/client'
import { faker } from '@faker-js/faker'

import './index.css'

import {
  ColumnDef,
  ColumnOrderState,
  flexRender,
  getCoreRowModel,
  useReactTable,
} from '@tanstack/react-table'
import { makeData, Person } from './makeData'

const defaultColumns: ColumnDef<Person>[] = [
  {
    header: 'Name',
    footer: props => props.column.id,
    columns: [
      {
        accessorKey: 'firstName',
        cell: info => info.getValue(),
        footer: props => props.column.id,
      },
      {
        accessorFn: row => row.lastName,
        id: 'lastName',
        cell: info => info.getValue(),
        header: () => <span>Last Name</span>,
        footer: props => props.column.id,
      },
    ],
  },
  {
    header: 'Info',
    footer: props => props.column.id,
    columns: [
      {
        accessorKey: 'age',
        header: () => 'Age',
        footer: props => props.column.id,
      },
      {
        header: 'More Info',
        columns: [
          {
            accessorKey: 'visits',
            header: () => <span>Visits</span>,
            footer: props => props.column.id,
          },
          {
            accessorKey: 'status',
            header: 'Status',
            footer: props => props.column.id,
          },
          {
            accessorKey: 'progress',
            header: 'Profile Progress',
            footer: props => props.column.id,
          },
        ],
      },
    ],
  },
]

function App() {
  const [data, setData] = React.useState(() => makeData(20))
  const [columns] = React.useState(() => [...defaultColumns])

  const [columnVisibility, setColumnVisibility] = React.useState({})
  const [columnOrder, setColumnOrder] = React.useState<ColumnOrderState>([])

  const rerender = () => setData(() => makeData(20))

  const table = useReactTable({
    data,
    columns,
    state: {
      columnVisibility,
      columnOrder,
    },
    onColumnVisibilityChange: setColumnVisibility,
    onColumnOrderChange: setColumnOrder,
    getCoreRowModel: getCoreRowModel(),
    debugTable: true,
    debugHeaders: true,
    debugColumns: true,
  })

  const randomizeColumns = () => {
    table.setColumnOrder(
      faker.helpers.shuffle(table.getAllLeafColumns().map(d => d.id))
    )
  }

  return (
    <div className="p-2">
      <div className="inline-block border border-black shadow rounded">
        <div className="px-1 border-b border-black">
          <label>
            <input
              {...{
                type: 'checkbox',
                checked: table.getIsAllColumnsVisible(),
                onChange: table.getToggleAllColumnsVisibilityHandler(),
              }}
            />{' '}
            Toggle All
          </label>
        </div>
        {table.getAllLeafColumns().map(column => {
          return (
            <div key={column.id} className="px-1">
              <label>
                <input
                  {...{
                    type: 'checkbox',
                    checked: column.getIsVisible(),
                    onChange: column.getToggleVisibilityHandler(),
                  }}
                />{' '}
                {column.id}
              </label>
            </div>
          )
        })}
      </div>
      <div className="h-4" />
      <div className="flex flex-wrap gap-2">
        <button onClick={() => rerender()} className="border p-1">
          Regenerate
        </button>
        <button onClick={() => randomizeColumns()} className="border p-1">
          Shuffle Columns
        </button>
      </div>
      <div className="h-4" />
      <table>
        <thead>
          {table.getHeaderGroups().map(headerGroup => (
            <tr key={headerGroup.id}>
              {headerGroup.headers.map(header => (
                <th key={header.id} colSpan={header.colSpan}>
                  {header.isPlaceholder
                    ? null
                    : flexRender(
                        header.column.columnDef.header,
                        header.getContext()
                      )}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map(row => (
            <tr key={row.id}>
              {row.getVisibleCells().map(cell => (
                <td key={cell.id}>
                  {flexRender(cell.column.columnDef.cell, cell.getContext())}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
        <tfoot>
          {table.getFooterGroups().map(footerGroup => (
            <tr key={footerGroup.id}>
              {footerGroup.headers.map(header => (
                <th key={header.id} colSpan={header.colSpan}>
                  {header.isPlaceholder
                    ? null
                    : flexRender(
                        header.column.columnDef.footer,
                        header.getContext()
                      )}
                </th>
              ))}
            </tr>
          ))}
        </tfoot>
      </table>
      <pre>{JSON.stringify(table.getState().columnOrder, null, 2)}</pre>
    </div>
  )
}

const rootElement = document.getElementById('root')
if (!rootElement) throw new Error('Failed to find the root element')

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
"###;

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let mut t = parser.tokenize_and_stem(code).unwrap();

        t.sort_by_key(|(t, _)| t.to_string());

        assert_ne!(t, vec![]);
    }

    #[test]
    fn test_3() {
        let code = r###"
const t = (
  <p>
    <strong>Notice:</strong> You are currently running React in
    development mode. Virtualized rendering performance will be slightly
    degraded until this application is built for production.
  </p>
)

"###;

        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let mut t = parser.tokenize_and_stem(code).unwrap();
        t.sort_by_key(|(t, _)| t.to_string());
        assert_eq!(t, vec![
            ("notice:".to_string(), vec![
                "notice".to_string()
            ]),
            ("p".to_string(), vec![]),
            ("p".to_string(), vec![]),
            ("strong".to_string(), vec![]),
            ("strong".to_string(), vec![]),
            ("t".to_string(), vec![]),
        ]);
    }

    #[test]
    fn test_4() {
        let code = r###"
        import React from 'react'
import ReactDOM from 'react-dom/client'
import { faker } from '@faker-js/faker'

import './index.css'

import {
  ColumnDef,
  ColumnOrderState,
  flexRender,
  getCoreRowModel,
  useReactTable,
  VisibilityState,
} from '@tanstack/react-table'
import { makeData, Person } from './makeData'

const defaultColumns: ColumnDef<Person>[] = [
  {
    header: 'Name',
    footer: props => props.column.id,
    columns: [
      {
        accessorKey: 'firstName',
        cell: info => info.getValue(),
        footer: props => props.column.id,
      },
      {
        accessorFn: row => row.lastName,
        id: 'lastName',
        cell: info => info.getValue(),
        header: () => <span>Last Name</span>,
        footer: props => props.column.id,
      },
    ],
  },
  {
    header: 'Info',
    footer: props => props.column.id,
    columns: [
      {
        accessorKey: 'age',
        header: () => 'Age',
        footer: props => props.column.id,
      },
      {
        header: 'More Info',
        columns: [
          {
            accessorKey: 'visits',
            header: () => <span>Visits</span>,
            footer: props => props.column.id,
          },
          {
            accessorKey: 'status',
            header: 'Status',
            footer: props => props.column.id,
          },
          {
            accessorKey: 'progress',
            header: 'Profile Progress',
            footer: props => props.column.id,
          },
        ],
      },
    ],
  },
]

function App() {
  const [data, setData] = React.useState(() => makeData(5000))
  const [columns] = React.useState(() => [...defaultColumns])

  const [columnVisibility, setColumnVisibility] =
    React.useState<VisibilityState>({})
  const [columnOrder, setColumnOrder] = React.useState<ColumnOrderState>([])
  const [columnPinning, setColumnPinning] = React.useState({})

  const [isSplit, setIsSplit] = React.useState(false)
  const rerender = () => setData(() => makeData(5000))

  const table = useReactTable({
    data,
    columns,
    state: {
      columnVisibility,
      columnOrder,
      columnPinning,
    },
    onColumnVisibilityChange: setColumnVisibility,
    onColumnOrderChange: setColumnOrder,
    onColumnPinningChange: setColumnPinning,
    getCoreRowModel: getCoreRowModel(),
    debugTable: true,
    debugHeaders: true,
    debugColumns: true,
  })

  const randomizeColumns = () => {
    table.setColumnOrder(
      faker.helpers.shuffle(table.getAllLeafColumns().map(d => d.id))
    )
  }

  return (
    <div className="p-2">
      <div className="inline-block border border-black shadow rounded">
        <div className="px-1 border-b border-black">
          <label>
            <input
              {...{
                type: 'checkbox',
                checked: table.getIsAllColumnsVisible(),
                onChange: table.getToggleAllColumnsVisibilityHandler(),
              }}
            />{' '}
            Toggle All
          </label>
        </div>
        {table.getAllLeafColumns().map(column => {
          return (
            <div key={column.id} className="px-1">
              <label>
                <input
                  {...{
                    type: 'checkbox',
                    checked: column.getIsVisible(),
                    onChange: column.getToggleVisibilityHandler(),
                  }}
                />{' '}
                {column.id}
              </label>
            </div>
          )
        })}
      </div>
      <div className="h-4" />
      <div className="flex flex-wrap gap-2">
        <button onClick={() => rerender()} className="border p-1">
          Regenerate
        </button>
        <button onClick={() => randomizeColumns()} className="border p-1">
          Shuffle Columns
        </button>
      </div>
      <div className="h-4" />
      <div>
        <label>
          <input
            type="checkbox"
            checked={isSplit}
            onChange={e => setIsSplit(e.target.checked)}
          />{' '}
          Split Mode
        </label>
      </div>
      <div className={`flex ${isSplit ? 'gap-4' : ''}`}>
        {isSplit ? (
          <table className="border-2 border-black">
            <thead>
              {table.getLeftHeaderGroups().map(headerGroup => (
                <tr key={headerGroup.id}>
                  {headerGroup.headers.map(header => (
                    <th key={header.id} colSpan={header.colSpan}>
                      <div className="whitespace-nowrap">
                        {header.isPlaceholder
                          ? null
                          : flexRender(
                              header.column.columnDef.header,
                              header.getContext()
                            )}
                      </div>
                      {!header.isPlaceholder && header.column.getCanPin() && (
                        <div className="flex gap-1 justify-center">
                          {header.column.getIsPinned() !== 'left' ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin('left')
                              }}
                            >
                              {'<='}
                            </button>
                          ) : null}
                          {header.column.getIsPinned() ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin(false)
                              }}
                            >
                              X
                            </button>
                          ) : null}
                          {header.column.getIsPinned() !== 'right' ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin('right')
                              }}
                            >
                              {'=>'}
                            </button>
                          ) : null}
                        </div>
                      )}
                    </th>
                  ))}
                </tr>
              ))}
            </thead>
            <tbody>
              {table
                .getRowModel()
                .rows.slice(0, 20)
                .map(row => {
                  return (
                    <tr key={row.id}>
                      {row.getLeftVisibleCells().map(cell => {
                        return (
                          <td key={cell.id}>
                            {flexRender(
                              cell.column.columnDef.cell,
                              cell.getContext()
                            )}
                          </td>
                        )
                      })}
                    </tr>
                  )
                })}
            </tbody>
          </table>
        ) : null}
        <table className="border-2 border-black">
          <thead>
            {(isSplit
              ? table.getCenterHeaderGroups()
              : table.getHeaderGroups()
            ).map(headerGroup => (
              <tr key={headerGroup.id}>
                {headerGroup.headers.map(header => (
                  <th key={header.id} colSpan={header.colSpan}>
                    <div className="whitespace-nowrap">
                      {header.isPlaceholder
                        ? null
                        : flexRender(
                            header.column.columnDef.header,
                            header.getContext()
                          )}
                    </div>
                    {!header.isPlaceholder && header.column.getCanPin() && (
                      <div className="flex gap-1 justify-center">
                        {header.column.getIsPinned() !== 'left' ? (
                          <button
                            className="border rounded px-2"
                            onClick={() => {
                              header.column.pin('left')
                            }}
                          >
                            {'<='}
                          </button>
                        ) : null}
                        {header.column.getIsPinned() ? (
                          <button
                            className="border rounded px-2"
                            onClick={() => {
                              header.column.pin(false)
                            }}
                          >
                            X
                          </button>
                        ) : null}
                        {header.column.getIsPinned() !== 'right' ? (
                          <button
                            className="border rounded px-2"
                            onClick={() => {
                              header.column.pin('right')
                            }}
                          >
                            {'=>'}
                          </button>
                        ) : null}
                      </div>
                    )}
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody>
            {table
              .getRowModel()
              .rows.slice(0, 20)
              .map(row => {
                return (
                  <tr key={row.id}>
                    {(isSplit
                      ? row.getCenterVisibleCells()
                      : row.getVisibleCells()
                    ).map(cell => {
                      return (
                        <td key={cell.id}>
                          {flexRender(
                            cell.column.columnDef.cell,
                            cell.getContext()
                          )}
                        </td>
                      )
                    })}
                  </tr>
                )
              })}
          </tbody>
        </table>
        {isSplit ? (
          <table className="border-2 border-black">
            <thead>
              {table.getRightHeaderGroups().map(headerGroup => (
                <tr key={headerGroup.id}>
                  {headerGroup.headers.map(header => (
                    <th key={header.id} colSpan={header.colSpan}>
                      <div className="whitespace-nowrap">
                        {header.isPlaceholder
                          ? null
                          : flexRender(
                              header.column.columnDef.header,
                              header.getContext()
                            )}
                      </div>
                      {!header.isPlaceholder && header.column.getCanPin() && (
                        <div className="flex gap-1 justify-center">
                          {header.column.getIsPinned() !== 'left' ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin('left')
                              }}
                            >
                              {'<='}
                            </button>
                          ) : null}
                          {header.column.getIsPinned() ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin(false)
                              }}
                            >
                              X
                            </button>
                          ) : null}
                          {header.column.getIsPinned() !== 'right' ? (
                            <button
                              className="border rounded px-2"
                              onClick={() => {
                                header.column.pin('right')
                              }}
                            >
                              {'=>'}
                            </button>
                          ) : null}
                        </div>
                      )}
                    </th>
                  ))}
                </tr>
              ))}
            </thead>
            <tbody>
              {table
                .getRowModel()
                .rows.slice(0, 20)
                .map(row => {
                  return (
                    <tr key={row.id}>
                      {row.getRightVisibleCells().map(cell => {
                        return (
                          <td key={cell.id}>
                            {flexRender(
                              cell.column.columnDef.cell,
                              cell.getContext()
                            )}
                          </td>
                        )
                      })}
                    </tr>
                  )
                })}
            </tbody>
          </table>
        ) : null}
      </div>
      <pre>{JSON.stringify(table.getState().columnPinning, null, 2)}</pre>
    </div>
  )
}

const rootElement = document.getElementById('root')
if (!rootElement) throw new Error('Failed to find the root element')

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
"###;
        let parser = CodeParser::from_language(CodeLanguage::TSX);

        let mut t = parser.tokenize_and_stem(code).unwrap();
        t.sort_by_key(|(t, _)| t.to_string());
        assert_ne!(t, vec![]);

        println!("{:#?}", t);
    }
}
