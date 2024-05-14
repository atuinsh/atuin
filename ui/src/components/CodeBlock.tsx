import { Highlight, themes } from "prism-react-renderer";
import Prism from "prismjs";
import "prismjs/components/prism-bash";

export default function CodeBlock({ code, language }: any) {
  return (
    <div className="overflow-auto">
      <Highlight
        theme={themes.github}
        code={code}
        prism={Prism}
        language={language}
      >
        {({ className, style, tokens, getLineProps, getTokenProps }) => (
          <pre style={style} className="p-4 break-words whitespace-pre-wrap">
            {tokens.map((line, i) => (
              <div key={i} {...getLineProps({ line })} data-vaul-no-drag>
                {i == 0 && (
                  <span className="text-gray-500 select-none">$ </span>
                )}
                {line.map((token, key) => (
                  <span
                    key={key}
                    {...getTokenProps({ token })}
                    data-vaul-no-drag
                  />
                ))}
              </div>
            ))}
          </pre>
        )}
      </Highlight>
    </div>
  );
}
