export default [
    {
        files: ['static/**/*.js'],
        languageOptions: {
            ecmaVersion: 'latest',
            sourceType: 'script',
            globals: {
                console: 'readonly',
                document: 'readonly',
                window: 'readonly',
                setTimeout: 'readonly',
                clearTimeout: 'readonly',
                setInterval: 'readonly',
                Date: 'readonly',
                Math: 'readonly',
                JSON: 'readonly',
                URLSearchParams: 'readonly',
                URL: 'readonly',
                Image: 'readonly',
                fetch: 'readonly',
                Element: 'readonly',
                Event: 'readonly',
                sessionStorage: 'readonly',
                requestAnimationFrame: 'readonly'
            }
        },
        rules: {
            'no-redeclare': 'error',
            'no-shadow': 'error',
            'no-unused-vars': ['error', {
                vars: 'all',
                args: 'after-used',
                ignoreRestSiblings: false
            }],
            'no-undef': 'error',
            'eqeqeq': ['error', 'always'],
            'curly': ['error', 'all'],
            'brace-style': ['error', '1tbs'],
            'indent': ['error', 4],
            'quotes': ['error', 'single', { avoidEscape: true, allowTemplateLiterals: true }],
            'semi': ['error', 'always']
        }
    },
    {
        ignores: ['node_modules/', 'target/', 'result/', '.direnv/']
    }
];
