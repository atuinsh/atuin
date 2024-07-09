import Logo from "@/assets/logo-light.svg";
import { useState } from "react";

import { login, register } from "@/state/client";
import { useStore } from "@/state/store";

interface LoginProps {
  toggleRegister: () => void;
  onClose: () => void;
}

function Login(props: LoginProps) {
  const refreshUser = useStore((state) => state.refreshUser);
  const [errors, setErrors] = useState<string | null>(null);

  const doLogin = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();

    const form = e.currentTarget;
    const username = form.username.value;
    const password = form.password.value;
    const key = form.key.value;

    console.log("Logging in...");
    try {
      await login(username, password, key);
      refreshUser();
      props.onClose();
    } catch (e: any) {
      console.error(e);
      setErrors(e);
    }
  };

  return (
    <>
      <div className="flex min-h-full flex-1 flex-col justify-center px-6 ">
        <div className="sm:mx-auto sm:w-full sm:max-w-sm">
          <img className="mx-auto h-10 w-auto" src={Logo} alt="Atuin" />

          <h2 className="mt-5 text-center text-2xl font-bold leading-9 tracking-tight text-gray-900">
            Sign in to your account
          </h2>

          <p className="text-sm text-center text-gray-600 mt-4 text-wrap">
            Backup and sync your data across devices. All data is end-to-end
            encrypted and stored securely in the cloud.
          </p>
        </div>

        <div className="mt-10 sm:mx-auto sm:w-full sm:max-w-sm">
          <form
            className="space-y-6"
            action="#"
            method="POST"
            onSubmit={doLogin}
          >
            <div>
              <label
                htmlFor="username"
                className="block text-sm font-medium leading-6 text-gray-900"
              >
                Username
              </label>
              <div className="mt-2">
                <input
                  id="username"
                  name="username"
                  type="username"
                  autoComplete="off"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 outline-none text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <div className="flex items-center justify-between">
                <label
                  htmlFor="password"
                  className="block text-sm font-medium leading-6 text-gray-900"
                >
                  Password
                </label>
                <div className="text-sm">
                  {/* You can't right now. Sorry. Validate emails first.
                  <a
                    href="#"
                    className="font-semibold text-emerald-600 hover:text-emerald-500"
                  >
                    Forgot password?
                  </a>
                  */}
                </div>
              </div>
              <div className="mt-2">
                <input
                  id="password"
                  name="password"
                  type="password"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  autoComplete="current-password"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset outline-none focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <div className="flex items-center justify-between">
                <label
                  htmlFor="key"
                  className="block text-sm font-medium leading-6 text-gray-900"
                >
                  <p>Key</p>
                  <p className="text-xs text-gray-500 font-normal">
                    Paste the output of "atuin key" from another machine
                  </p>
                </label>
              </div>
              <div className="mt-2">
                <input
                  id="key"
                  name="key"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  autoComplete="off"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset outline-none focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <button
                type="submit"
                className="flex w-full justify-center rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-semibold leading-6 text-white shadow-sm hover:bg-emerald-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-emerald-600"
              >
                Sign in
              </button>
            </div>
          </form>

          {errors && (
            <p className="mt-4 text-center text-sm text-red-500">{errors}</p>
          )}

          <p className="mt-10 text-center text-sm text-gray-500">
            Not a member?{" "}
            <a
              href="#"
              className="font-semibold leading-6 text-emerald-600 hover:text-emerald-500"
              onClick={(e) => {
                e.preventDefault();
                props.toggleRegister();
              }}
            >
              Register
            </a>
          </p>
        </div>
      </div>
    </>
  );
}

interface RegisterProps {
  toggleLogin: () => void;
  onClose: () => void;
}

function Register(props: RegisterProps) {
  const refreshUser = useStore((state) => state.refreshUser);
  const [errors, setErrors] = useState<string | null>(null);

  const doRegister = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();

    const form = e.currentTarget;
    const username = form.username.value;
    const email = form.email.value;
    const password = form.password.value;

    try {
      await register(username, email, password);
      refreshUser();
      props.onClose();
    } catch (e: any) {
      setErrors(e);
    }
  };

  return (
    <>
      <div className="flex min-h-full flex-1 flex-col justify-center px-6 ">
        <div className="sm:mx-auto sm:w-full sm:max-w-sm">
          <img className="mx-auto h-10 w-auto" src={Logo} alt="Atuin" />

          <h2 className="mt-5 text-center text-2xl font-bold leading-9 tracking-tight text-gray-900">
            Register for an account
          </h2>

          <p className="text-sm text-center text-gray-600 mt-4 text-wrap">
            Backup and sync your data across devices. All data is end-to-end
            encrypted and stored securely in the cloud.
          </p>
        </div>

        <div className="mt-10 sm:mx-auto sm:w-full sm:max-w-sm">
          <form
            className="space-y-6"
            action="#"
            method="POST"
            onSubmit={doRegister}
          >
            <div>
              <label
                htmlFor="username"
                className="block text-sm font-medium leading-6 text-gray-900"
              >
                Username
              </label>
              <div className="mt-2">
                <input
                  id="username"
                  name="username"
                  type="username"
                  autoComplete="off"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 outline-none text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <label
                htmlFor="email"
                className="block text-sm font-medium leading-6 text-gray-900"
              >
                Email
              </label>
              <div className="mt-2">
                <input
                  id="email"
                  name="email"
                  type="email"
                  autoComplete="email"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 outline-none text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <div className="flex items-center justify-between">
                <label
                  htmlFor="password"
                  className="block text-sm font-medium leading-6 text-gray-900"
                >
                  Password
                </label>
                <div className="text-sm">
                  {/* You can't right now. Sorry. Validate emails first.
                  <a
                    href="#"
                    className="font-semibold text-emerald-600 hover:text-emerald-500"
                  >
                    Forgot password?
                  </a>
                  */}
                </div>
              </div>
              <div className="mt-2">
                <input
                  id="password"
                  name="password"
                  type="password"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck="false"
                  autoComplete="current-password"
                  required
                  className="block w-full rounded-md border-0 px-1.5 py-1.5 text-gray-900 shadow-sm ring-1 ring-inset ring-gray-300 placeholder:text-gray-400 focus:ring-2 focus:ring-inset outline-none focus:ring-emerald-600 sm:text-sm sm:leading-6"
                />
              </div>
            </div>

            <div>
              <button
                type="submit"
                className="flex w-full justify-center rounded-md bg-emerald-600 px-3 py-1.5 text-sm font-semibold leading-6 text-white shadow-sm hover:bg-emerald-500 focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-emerald-600"
              >
                Register
              </button>
            </div>
          </form>

          {errors && (
            <p className="mt-4 text-center text-sm text-red-500">{errors}</p>
          )}

          <p className="mt-10 text-center text-sm text-gray-500">
            Already have an account?{" "}
            <a
              href="#"
              className="font-semibold leading-6 text-emerald-600 hover:text-emerald-500"
              onClick={(e) => {
                e.preventDefault();
                props.toggleLogin();
              }}
            >
              Login
            </a>
          </p>
        </div>
      </div>
    </>
  );
}

export default function LoginOrRegister({ onClose }: { onClose: () => void }) {
  let [login, setLogin] = useState<boolean>(false);

  if (login) {
    return <Login onClose={onClose} toggleRegister={() => setLogin(false)} />;
  }

  return <Register onClose={onClose} toggleLogin={() => setLogin(true)} />;
}
