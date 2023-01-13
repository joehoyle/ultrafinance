import { Link, ScrollRestoration } from "react-router-dom";
import Footer from "../components/Footer";

export default function DataCompliance() {
	return <>
		<ScrollRestoration />
		<div className="p-8 flex flex-col from-purple-700 to-purple-900 bg-gradient-to-t">
			<div className="text-white flex justify-between">
				<Link to="/">
					<svg width="40px" height="40px" viewBox="0 0 300 300">
						<defs>
							<clipPath id="Group">
								<polygon points="156.040268 4.54747351e-13 206.375839 4.54747351e-13 206.375839 195.06897 156.040268 300"></polygon>
								<polygon transform="translate(234.060403, 25.000000) rotate(-270.000000) translate(-234.060403, -25.000000) " points="209.060403 -40.9395973 259.060403 -16.9210754 259.060403 90.9395973 209.060403 90.9395973"></polygon>
								<polygon transform="translate(213.963533, 115.000000) rotate(-270.000000) translate(-213.963533, -115.000000) " points="188.963533 72.2521235 238.963533 96.2706454 238.963533 157.747876 188.963533 157.747876"></polygon>
								<polygon points="93.6241611 4.54747351e-13 143.959732 4.54747351e-13 143.959732 300 97.147651 173.03125"></polygon>
								<polygon points="0 4.54747351e-13 50.3355705 4.54747351e-13 139.563949 185.944453 143.959732 300"></polygon>
							</clipPath>
						</defs>
						<rect clip-path="url(#Group)" id="Rectangle" fill="currentColor" x="0" y="0" width="300" height="300"></rect>
					</svg>
				</Link>

				<div className="flex space-x-4 items-center font-semibold text-white">
					<Link className="p-2" to="/login">Log In</Link>
					<Link className="p-2 px-4 bg-white/20 rounded-lg" to="/signup">Sign Up</Link>
				</div>
			</div>
		</div>

		<h1 className="mt-12 text-3xl text-center font-bold tracking-tight text-gray-900">Data Compliance</h1>

		<div className="p-8 max-w-4xl mx-auto mt-8">
			<h3 className="text-gray-700 text-2xl font-semibold">PSD2</h3>
			<p className="my-7 text-xl text-gray-500">
				Ultrafinance is classified as an AISP (Account Information Service Provider) which falls under the PSD2 (Payment Services Directive 2) regulation. There is also overlap with the GDPR, as individual's bank account information is considered Personal Data.
			</p>
			<p className="my-7 text-xl text-gray-500">
				Ultrafinance is not directly licensed as an AISP, instead Ultrafinance uses <a className="text-purple-600" href="https://nordigen.com/en/">Nordigen</a> to provide Bank Account information services and connection. Nordigen is a licensed AISP by the Financial and Capital Market Commission in Latvia.
			</p>
			<p className="my-7 text-xl text-gray-500">
				You can check Nordigen's licensing validity in the <a className="text-purple-600" href="https://euclid.eba.europa.eu/register/pir/search">European Banking Authority's (EBA) Payment Institutions Register</a>.
			</p>
			<h3 className="text-gray-700 text-2xl font-semibold mt-16">GDPR</h3>
			<p className="my-7 text-xl text-gray-500">
				Ultrafinance is a Data Controller for your account information under the GDPR, which consists of your account's data (such as balances and transactions) as is your account name and email address.
			</p>
			<p className="my-7 text-xl text-gray-500">
				Ultrafinance allows you to access all of your data, and delete all data at your request. We collect your Name and Email address as part of registration, and allow you to connect Nordigen to your bank account. You can revoke permissions to your accounts at any time, and delete data from Ultrafinance via the Ultrafinance web application.
			</p>
			<h3 className="text-gray-700 text-2xl font-semibold mt-16">Sub-Processors</h3>
			<p className="my-7 mb-14 text-xl text-gray-500">
				Ultrafinance uses <a className="text-purple-600" href="https://aws.amazon.com/">Amazon Web Services</a> for Cloud Compute, storage and data processing services. Ultrafinance hosts data exclusively within the Frankfurt region, which is in the EU.
			</p>
		</div>

		<Footer />
	</>
}
